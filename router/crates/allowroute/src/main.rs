//! DNS failover in front of a GlobalProtect tunnel.
//!
//! System DNS is asked first. A public answer is returned unchanged.
//! Any other result is asked again through the tunnel DNS, and the answered
//! addresses are routed via the tunnel device. `10.0.0.0/8` is also routed
//! via the tunnel so a raw address in that range does not need a name.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use allowroute::{decide, host_route, inspect_dns, prefix_covers};

const MARK: libc::c_int = 0x6770;
const LISTEN_PORT: u16 = 5353;
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

struct Args {
    tun: String,
    tunnel_dns: Vec<IpAddr>,
    prefixes: Vec<String>,
    state_dir: PathBuf,
}

struct Shared {
    tun: String,
    state_dir: PathBuf,
    prefixes: Vec<String>,
    routes: Mutex<Vec<String>>,
}

fn main() {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    match cmd.as_str() {
        "run" => {
            if let Err(err) = run(parse_args(args)) {
                eprintln!("allowroute: {err}");
                std::process::exit(1);
            }
        }
        "down" => {
            let state_dir = state_dir_from(args);
            let _ = cleanup(&state_dir);
        }
        _ => {
            eprintln!("usage: allowroute run --tun DEV --tunnel-dns IP[,IP] [--prefix CIDR] [--state-dir DIR]");
            eprintln!("       allowroute down [--state-dir DIR]");
            std::process::exit(2);
        }
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Args {
    let mut tun = String::new();
    let mut tunnel_dns = Vec::new();
    let mut prefixes = Vec::new();
    let mut state_dir = PathBuf::from("/run/gpclient/allowroute");
    let mut tokens = args.peekable();
    while let Some(token) = tokens.next() {
        match token.as_str() {
            "--tun" => tun = tokens.next().unwrap_or_default(),
            "--tunnel-dns" => tunnel_dns.extend(split_ips(&tokens.next().unwrap_or_default())),
            "--prefix" => prefixes.push(tokens.next().unwrap_or_default()),
            "--state-dir" => state_dir = PathBuf::from(tokens.next().unwrap_or_default()),
            other if other.starts_with("--tunnel-dns=") => {
                tunnel_dns.extend(split_ips(other.trim_start_matches("--tunnel-dns=")));
            }
            other if other.starts_with("--prefix=") => {
                prefixes.push(other.trim_start_matches("--prefix=").to_string());
            }
            other if other.starts_with("--state-dir=") => {
                state_dir = PathBuf::from(other.trim_start_matches("--state-dir="));
            }
            other if other.starts_with("--tun=") => tun = other.trim_start_matches("--tun=").to_string(),
            _ => {}
        }
    }
    if prefixes.is_empty() {
        let from_env = env::var("GP_VPN_PREFIXES").unwrap_or_else(|_| "10.0.0.0/8".to_string());
        prefixes.extend(from_env.split(|c: char| c == ',' || c.is_whitespace()).filter(|part| !part.is_empty()).map(str::to_string));
    }
    Args { tun, tunnel_dns, prefixes, state_dir }
}

fn state_dir_from(args: impl Iterator<Item = String>) -> PathBuf {
    parse_args(args).state_dir
}

fn split_ips(text: &str) -> Vec<IpAddr> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn run(args: Args) -> io::Result<()> {
    if args.tun.is_empty() || !is_dev_name(&args.tun) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "missing --tun"));
    }
    install_signals();
    fs::create_dir_all(&args.state_dir)?;
    let shared = Arc::new(Shared {
        tun: args.tun.clone(),
        state_dir: args.state_dir.clone(),
        prefixes: args.prefixes.clone(),
        routes: Mutex::new(Vec::new()),
    });
    for prefix in &args.prefixes {
        if !is_prefix(prefix) {
            eprintln!("allowroute: ignoring prefix {prefix}");
            continue;
        }
        add_route(prefix, &shared.tun)?;
        remember(&shared, prefix);
    }
    for dns in &args.tunnel_dns {
        if args.prefixes.iter().any(|prefix| prefix_covers(prefix, *dns)) {
            continue;
        }
        let cidr = host_route(*dns);
        add_route(&cidr, &shared.tun)?;
        remember(&shared, &cidr);
    }
    if !install_redirect() {
        eprintln!("allowroute: DNS redirect was not installed; aggregate routing is still active");
    }
    let udp = UdpSocket::bind(("127.0.0.1", LISTEN_PORT))?;
    udp.set_read_timeout(Some(Duration::from_millis(200)))?;
    let tcp = TcpListener::bind(("127.0.0.1", LISTEN_PORT))?;
    tcp.set_nonblocking(true)?;
    eprintln!(
        "allowroute: tun {} prefixes {} tunnel-dns {}",
        args.tun,
        args.prefixes.join(","),
        args.tunnel_dns.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
    );
    let tcp_shared = Arc::clone(&shared);
    let tunnel_dns = args.tunnel_dns.clone();
    thread::spawn(move || tcp_loop(tcp, tcp_shared, tunnel_dns));
    while !STOP.load(Ordering::Relaxed) {
        let mut buf = [0u8; 4096];
        match udp.recv_from(&mut buf) {
            Ok((len, src)) => {
                let reply = handle_query(&buf[..len], &shared, &args.tunnel_dns);
                let _ = udp.send_to(&reply, src);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::TimedOut => {}
            Err(err) => eprintln!("allowroute: udp: {err}"),
        }
    }
    cleanup(&args.state_dir)
}

fn tcp_loop(listener: TcpListener, shared: Arc<Shared>, tunnel_dns: Vec<IpAddr>) {
    while !STOP.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let shared = Arc::clone(&shared);
                let tunnel_dns = tunnel_dns.clone();
                thread::spawn(move || {
                    if let Some(query) = read_tcp_dns(&mut stream) {
                        let reply = handle_query(&query, &shared, &tunnel_dns);
                        let _ = write_tcp_dns(&mut stream, &reply);
                    }
                });
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                eprintln!("allowroute: tcp: {err}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn handle_query(query: &[u8], shared: &Shared, tunnel_dns: &[IpAddr]) -> Vec<u8> {
    let system = system_resolvers();
    let system_resp = query_dns(query, &system);
    let system_addrs = system_resp.as_ref().and_then(|pkt| inspect_dns(pkt)).map(|view| view.answers).unwrap_or_default();
    let looked = decide(&system_addrs, &[]);
    if !looked.addresses.is_empty() && !looked.use_tunnel {
        return system_resp.unwrap_or_else(|| servfail(query));
    }
    let tunnel_resp = query_dns(query, &socket_addrs(tunnel_dns));
    let tunnel_addrs = tunnel_resp.as_ref().and_then(|pkt| inspect_dns(pkt)).map(|view| view.answers).unwrap_or_default();
    let looked = decide(&system_addrs, &tunnel_addrs);
    if looked.use_tunnel {
        install_answers(&looked.addresses, shared);
        if let Some(pkt) = tunnel_resp {
            return pkt;
        }
    }
    system_resp.unwrap_or_else(|| servfail(query))
}

fn remember(shared: &Shared, cidr: &str) {
    let mut guard = shared.routes.lock().expect("routes");
    if !guard.iter().any(|have| have == cidr) {
        guard.push(cidr.to_string());
    }
    let _ = write_routes(&shared.state_dir, &shared.tun, &guard);
}

fn install_answers(addrs: &[IpAddr], shared: &Shared) {
    for addr in addrs {
        if shared.prefixes.iter().any(|prefix| prefix_covers(prefix, *addr)) {
            continue;
        }
        let cidr = host_route(*addr);
        if add_route(&cidr, &shared.tun).is_ok() {
            remember(shared, &cidr);
        }
    }
}

fn query_dns(query: &[u8], servers: &[SocketAddr]) -> Option<Vec<u8>> {
    for server in servers {
        if let Some(pkt) = query_one(query, *server) {
            return Some(pkt);
        }
    }
    None
}

fn query_one(query: &[u8], server: SocketAddr) -> Option<Vec<u8>> {
    let udp = query_udp(query, server)?;
    if inspect_truncated(&udp) {
        return query_tcp(query, server).or(Some(udp));
    }
    Some(udp)
}

fn inspect_truncated(packet: &[u8]) -> bool {
    packet.len() >= 4 && packet[2] & 0x02 != 0
}

fn query_udp(query: &[u8], server: SocketAddr) -> Option<Vec<u8>> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse().ok()? } else { "[::]:0".parse().ok()? };
    let sock = UdpSocket::bind(bind).ok()?;
    mark_fd(sock.as_raw_fd()).ok()?;
    sock.set_read_timeout(Some(QUERY_TIMEOUT)).ok()?;
    sock.send_to(query, server).ok()?;
    let mut buf = [0u8; 4096];
    let (len, _) = sock.recv_from(&mut buf).ok()?;
    if len < 12 || buf[0] != query[0] || buf[1] != query[1] {
        return None;
    }
    Some(buf[..len].to_vec())
}

fn query_tcp(query: &[u8], server: SocketAddr) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&server, QUERY_TIMEOUT).ok()?;
    mark_fd(stream.as_raw_fd()).ok()?;
    stream.set_read_timeout(Some(QUERY_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(QUERY_TIMEOUT)).ok()?;
    write_tcp_dns(&mut stream, query).ok()?;
    let pkt = read_tcp_dns(&mut stream)?;
    if pkt.len() < 2 || pkt[0] != query[0] || pkt[1] != query[1] {
        return None;
    }
    Some(pkt)
}

fn read_tcp_dns(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).ok()?;
    let len = u16::from_be_bytes(len_buf) as usize;
    if len > 8192 {
        return None;
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn write_tcp_dns(stream: &mut TcpStream, packet: &[u8]) -> io::Result<()> {
    let len = u16::try_from(packet.len()).unwrap_or(u16::MAX);
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&packet[..len as usize])?;
    Ok(())
}

fn servfail(query: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0, 0, 0x81, 0x82, 0, 0, 0, 0, 0, 0, 0, 0];
    if query.len() >= 2 {
        pkt[0] = query[0];
        pkt[1] = query[1];
    }
    pkt
}

fn socket_addrs(ips: &[IpAddr]) -> Vec<SocketAddr> {
    ips.iter().map(|ip| SocketAddr::new(*ip, 53)).collect()
}

fn system_resolvers() -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for path in ["/run/systemd/resolve/resolv.conf", "/etc/resolv.conf"] {
        let Ok(text) = fs::read_to_string(path) else { continue };
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() != Some("nameserver") {
                continue;
            }
            let Some(addr) = parts.next().and_then(|text| text.parse::<IpAddr>().ok()) else { continue };
            if is_loopback(addr) {
                continue;
            }
            let sock = SocketAddr::new(addr, 53);
            if !out.contains(&sock) {
                out.push(sock);
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

fn add_route(cidr: &str, tun: &str) -> io::Result<()> {
    let family = if cidr.contains(':') { "-6" } else { "-4" };
    let status = Command::new(ip_bin()).args([family, "route", "replace", cidr, "dev", tun]).status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("ip route replace {cidr} failed"))) }
}

fn del_route(cidr: &str, tun: &str) {
    let family = if cidr.contains(':') { "-6" } else { "-4" };
    let _ = Command::new(ip_bin()).args([family, "route", "del", cidr, "dev", tun]).status();
}

fn install_redirect() -> bool {
    let program = format!(
        "table inet gpallow {{
  chain output {{
    type nat hook output priority dstnat; policy accept;
    meta mark {MARK:#x} return
    udp dport 53 redirect to :{LISTEN_PORT}
    tcp dport 53 redirect to :{LISTEN_PORT}
  }}
}}
"
    );
    let path = "/run/gpclient/allowroute.nft";
    if fs::create_dir_all("/run/gpclient").is_err() || fs::write(path, program).is_err() {
        return false;
    }
    let _ = Command::new(nft_bin()).args(["delete", "table", "inet", "gpallow"]).status();
    Command::new(nft_bin()).args(["-f", path]).status().map(|s| s.success()).unwrap_or(false)
}

fn remove_redirect() {
    let _ = Command::new(nft_bin()).args(["delete", "table", "inet", "gpallow"]).status();
    let _ = fs::remove_file("/run/gpclient/allowroute.nft");
}

fn cleanup(state_dir: &Path) -> io::Result<()> {
    remove_redirect();
    let path = state_dir.join("routes");
    if let Ok(text) = fs::read_to_string(&path) {
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(cidr) = parts.next() else { continue };
            let Some(tun) = parts.next() else { continue };
            if is_prefix(cidr) && is_dev_name(tun) {
                del_route(cidr, tun);
            }
        }
    }
    let _ = fs::remove_file(path);
    Ok(())
}

fn write_routes(dir: &Path, tun: &str, routes: &[String]) -> io::Result<()> {
    let body: String = routes.iter().map(|cidr| format!("{cidr} {tun}\n")).collect();
    fs::create_dir_all(dir)?;
    fs::write(dir.join("routes"), body)
}

fn install_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as libc::sighandler_t);
    }
}

fn mark_fd(fd: std::os::fd::RawFd) -> io::Result<()> {
    let mark = MARK;
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_MARK,
            &mark as *const libc::c_int as *const libc::c_void,
            std::mem::size_of_val(&mark) as libc::socklen_t,
        )
    };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn ip_bin() -> &'static str {
    if Path::new("/sbin/ip").exists() { "/sbin/ip" } else { "ip" }
}

fn nft_bin() -> &'static str {
    if Path::new("/usr/sbin/nft").exists() { "/usr/sbin/nft" } else { "nft" }
}

fn is_dev_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 16
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn is_prefix(text: &str) -> bool {
    let Some((addr, len)) = text.split_once('/') else { return false };
    let Ok(len) = len.parse::<u8>() else { return false };
    match addr.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => len <= 32,
        Ok(IpAddr::V6(_)) => len <= 128,
        Err(_) => false,
    }
}
