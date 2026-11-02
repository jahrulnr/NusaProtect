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
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use allowroute::{build_query, decide, host_route, inspect_dns, is_public_unicast, prefix_covers, route_uses_dev};

const MARK: libc::c_int = 0x6770;
/// Not 5353: that port is held by avahi-daemon (mDNS) on many desktops.
const LISTEN_PORT: u16 = 15353;
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
/// In-flight DNS queries handled concurrently; excess datagrams are dropped
/// and the client retries, which is normal DNS behaviour.
const MAX_INFLIGHT: usize = 64;
const DEFAULT_STATE_DIR: &str = "/run/npclient/allowroute";
const RUNTIME_DIR: &str = "/run/npclient";
const NFT_FILE: &str = "/run/npclient/allowroute.nft";
const PID_FILE: &str = "allowroute.pid";
/// How often the sweeper looks for expired DNS-derived routes.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);
/// A DNS-derived tunnel route is re-validated after this long. A name that now
/// resolves publicly loses its tunnel route so it cannot get stuck behind the
/// VPN after one bad answer. `0` keeps routes until disconnect.
const DEFAULT_ROUTE_TTL: Duration = Duration::from_secs(300);
/// One year. Absurd for DNS, but keeps `Instant::now() + ttl` from overflowing.
const MAX_ROUTE_TTL_SECS: u64 = 31_536_000;

static STOP: AtomicBool = AtomicBool::new(false);
static CLEAR: AtomicBool = AtomicBool::new(false);
static QUERY_ID: AtomicU16 = AtomicU16::new(0x4e50);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_clear(_: libc::c_int) {
    CLEAR.store(true, Ordering::Relaxed);
}

struct Args {
    tun: String,
    tunnel_dns: Vec<IpAddr>,
    prefixes: Vec<String>,
    state_dir: PathBuf,
    route_ttl: Duration,
}

/// A resolved name that produced tunnel host routes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    name: String,
    qtype: u16,
}

#[derive(Debug)]
struct TrackedRoute {
    cidrs: Vec<String>,
    expires: Instant,
}

/// DNS-derived tunnel state: host routes grouped by the name that produced
/// them, plus a refcount so two names sharing an address keep the route until
/// both are gone.
#[derive(Default)]
struct RouteState {
    by_name: HashMap<RouteKey, TrackedRoute>,
    refs: HashMap<String, usize>,
}

/// Kernel route I/O so the bookkeeping logic can run against a fake in tests.
trait RouteCtl: Send + Sync {
    fn add(&self, cidr: &str) -> io::Result<()>;
    fn del(&self, cidr: &str);
}

/// Upstream DNS transport so query flows can be simulated in tests.
trait DnsIo: Send + Sync {
    fn query(&self, pkt: &[u8], servers: &[SocketAddr]) -> Option<Vec<u8>>;
}

struct IpRoutes(String);
impl RouteCtl for IpRoutes {
    fn add(&self, cidr: &str) -> io::Result<()> {
        add_route(cidr, &self.0)
    }
    fn del(&self, cidr: &str) {
        del_route(cidr, &self.0);
    }
}

struct UdpDns;
impl DnsIo for UdpDns {
    fn query(&self, pkt: &[u8], servers: &[SocketAddr]) -> Option<Vec<u8>> {
        query_dns(pkt, servers)
    }
}

struct Shared {
    tun: String,
    tun_addrs: Vec<IpAddr>,
    state_dir: PathBuf,
    prefixes: Vec<String>,
    tunnel_dns: Vec<IpAddr>,
    route_ttl: Duration,
    /// Routes installed outside DNS answers (VPN prefixes, tunnel-DNS host
    /// routes). A tracked name that claims the same CIDR must not delete it
    /// when released.
    static_routes: HashSet<String>,
    routes: Mutex<Vec<String>>,
    route_state: Mutex<RouteState>,
    skipped_dns: Mutex<HashSet<IpAddr>>,
    routes_io: Arc<dyn RouteCtl>,
    dns_io: Arc<dyn DnsIo>,
    get_system_resolvers: Arc<dyn Fn(&Shared) -> Vec<SocketAddr> + Send + Sync>,
    iface_up: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

/// Returns the in-flight slot on drop, even when the worker panics.
struct Inflight(Arc<AtomicUsize>);

impl Drop for Inflight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
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
        "clear" => {
            let state_dir = state_dir_from(args);
            if let Err(err) = clear(&state_dir) {
                eprintln!("allowroute: {err}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: allowroute run --tun DEV --tunnel-dns IP[,IP] [--prefix CIDR] [--route-ttl SECS] [--state-dir DIR]");
            eprintln!("       allowroute down [--state-dir DIR]");
            eprintln!("       allowroute clear [--state-dir DIR]");
            std::process::exit(2);
        }
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Args {
    let mut tun = String::new();
    let mut tunnel_dns = Vec::new();
    let mut prefixes = Vec::new();
    let mut state_dir = PathBuf::from(DEFAULT_STATE_DIR);
    let mut route_ttl = env::var("NP_DNS_ROUTE_TTL")
        .ok()
        .and_then(|v| route_ttl_secs(&v))
        .unwrap_or(DEFAULT_ROUTE_TTL);
    let mut tokens = args.peekable();
    while let Some(token) = tokens.next() {
        match token.as_str() {
            "--tun" => tun = tokens.next().unwrap_or_default(),
            "--tunnel-dns" => tunnel_dns.extend(split_ips(&tokens.next().unwrap_or_default())),
            "--prefix" => prefixes.push(tokens.next().unwrap_or_default()),
            "--state-dir" => state_dir = PathBuf::from(tokens.next().unwrap_or_default()),
            "--route-ttl" => match tokens.peek().and_then(|v| route_ttl_secs(v)) {
                Some(ttl) => {
                    tokens.next();
                    route_ttl = ttl;
                }
                // Peeked, so the next flag is not swallowed by mistake.
                None => eprintln!("allowroute: --route-ttl needs a value in seconds; keeping {route_ttl:?}"),
            },
            other if other.starts_with("--tunnel-dns=") => {
                tunnel_dns.extend(split_ips(other.trim_start_matches("--tunnel-dns=")));
            }
            other if other.starts_with("--prefix=") => {
                prefixes.push(other.trim_start_matches("--prefix=").to_string());
            }
            other if other.starts_with("--state-dir=") => {
                state_dir = PathBuf::from(other.trim_start_matches("--state-dir="));
            }
            other if other.starts_with("--route-ttl=") => match route_ttl_secs(other.trim_start_matches("--route-ttl=")) {
                Some(ttl) => route_ttl = ttl,
                None => eprintln!("allowroute: invalid --route-ttl value; keeping {route_ttl:?}"),
            },
            other if other.starts_with("--tun=") => tun = other.trim_start_matches("--tun=").to_string(),
            _ => {}
        }
    }
    if prefixes.is_empty() {
        let from_env = env::var("NP_VPN_PREFIXES")
            .or_else(|_| env::var("GP_VPN_PREFIXES"))
            .unwrap_or_else(|_| "10.0.0.0/8".to_string());
        prefixes.extend(from_env.split(|c: char| c == ',' || c.is_whitespace()).filter(|part| !part.is_empty()).map(str::to_string));
    }
    Args { tun, tunnel_dns, prefixes, state_dir, route_ttl }
}

fn state_dir_from(args: impl Iterator<Item = String>) -> PathBuf {
    parse_args(args).state_dir
}

fn route_ttl_secs(text: &str) -> Option<Duration> {
    text.trim()
        .parse::<u64>()
        .ok()
        .map(|secs| Duration::from_secs(secs.min(MAX_ROUTE_TTL_SECS)))
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
    // Static routes installed outside DNS answers: VPN prefixes plus host
    // routes for tunnel DNS servers that the prefixes do not already cover.
    let mut static_routes = HashSet::new();
    for prefix in &args.prefixes {
        if !is_prefix(prefix) {
            eprintln!("allowroute: ignoring prefix {prefix}");
            continue;
        }
        static_routes.insert(prefix.clone());
    }
    for dns in &args.tunnel_dns {
        if args.prefixes.iter().any(|prefix| prefix_covers(prefix, *dns)) {
            continue;
        }
        static_routes.insert(host_route(*dns));
    }
    let shared = Arc::new(Shared {
        tun: args.tun.clone(),
        tun_addrs: iface_addrs(&args.tun),
        state_dir: args.state_dir.clone(),
        prefixes: args.prefixes.clone(),
        tunnel_dns: args.tunnel_dns.clone(),
        route_ttl: args.route_ttl,
        static_routes,
        routes: Mutex::new(Vec::new()),
        route_state: Mutex::new(RouteState::default()),
        skipped_dns: Mutex::new(HashSet::new()),
        routes_io: Arc::new(IpRoutes(args.tun.clone())),
        dns_io: Arc::new(UdpDns),
        get_system_resolvers: Arc::new(system_resolvers),
        iface_up: Arc::new(iface_exists),
    });
    let _ = fs::write(args.state_dir.join(PID_FILE), std::process::id().to_string());
    for cidr in &shared.static_routes {
        shared.routes_io.add(cidr)?;
        remember(&shared, cidr);
    }
    // Bind before installing nft redirect so a port conflict cannot leave DNS redirected
    // at a dead listener (previously common on desktops where avahi holds :5353).
    let udp = UdpSocket::bind(("127.0.0.1", LISTEN_PORT)).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("bind UDP 127.0.0.1:{LISTEN_PORT} failed: {err}"),
        )
    })?;
    udp.set_read_timeout(Some(Duration::from_millis(200)))?;
    let tcp = TcpListener::bind(("127.0.0.1", LISTEN_PORT)).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("bind TCP 127.0.0.1:{LISTEN_PORT} failed: {err}"),
        )
    })?;
    tcp.set_nonblocking(true)?;
    let redirect_installed = install_redirect();
    if !redirect_installed {
        let msg = "allowroute: DNS redirect was not installed; aggregate routing is still active";
        eprintln!("{msg}");
        append_log(&args.state_dir, msg);
    }
    let mut redirect_guard = RedirectGuard {
        active: redirect_installed,
    };
    let udp = Arc::new(udp);
    eprintln!(
        "allowroute: tun {} prefixes {} tunnel-dns {} system-dns {}",
        args.tun,
        args.prefixes.join(","),
        args.tunnel_dns.iter().map(ToString::to_string).collect::<Vec<_>>().join(","),
        system_resolvers(&shared)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    let tcp_shared = Arc::clone(&shared);
    thread::spawn(move || tcp_loop(tcp, tcp_shared));
    if shared.route_ttl > Duration::ZERO {
        let sweep_shared = Arc::clone(&shared);
        thread::spawn(move || sweeper(sweep_shared));
    }
    let inflight = Arc::new(AtomicUsize::new(0));
    let mut last_drop_notice = Instant::now() - Duration::from_secs(60);
    while !STOP.load(Ordering::Relaxed) {
        // Orphan watchdog: when the tunnel device is gone this daemon only
        // harms name resolution — exit so cleanup removes the DNS redirect.
        if orphaned(&shared) {
            eprintln!("allowroute: {} is gone; shutting down", shared.tun);
            break;
        }
        if CLEAR.swap(false, Ordering::Relaxed) {
            clear_tracked(&shared);
        }
        let mut buf = [0u8; 4096];
        match udp.recv_from(&mut buf) {
            Ok((len, src)) => {
                // One slow upstream must not serialize every client behind it.
                // Only this recv thread increments, so inflight never exceeds
                // MAX_INFLIGHT; excess datagrams drop and clients retry.
                if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
                    if last_drop_notice.elapsed() > Duration::from_secs(10) {
                        eprintln!("allowroute: dropping DNS queries: {MAX_INFLIGHT} already in flight");
                        last_drop_notice = Instant::now();
                    }
                    continue;
                }
                inflight.fetch_add(1, Ordering::Relaxed);
                let udp = Arc::clone(&udp);
                let shared = Arc::clone(&shared);
                let guard = Inflight(Arc::clone(&inflight));
                let query = buf[..len].to_vec();
                thread::spawn(move || {
                    let _guard = guard;
                    let reply = handle_query(&query, &shared);
                    let _ = udp.send_to(&reply, src);
                });
            }
            Err(err)
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::TimedOut
                    || err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => eprintln!("allowroute: udp: {err}"),
        }
    }
    // Normal shutdown: cleanup removes redirect; disarm so Drop is a no-op.
    let result = cleanup(&args.state_dir);
    redirect_guard.disarm();
    result
}

/// `clear` subcommand: ask the running daemon to drop every DNS-derived tunnel
/// route (names that fell over to the tunnel), then flush the OS resolver
/// cache so the next lookup re-decides from scratch.
fn clear(state_dir: &Path) -> io::Result<()> {
    let pid = fs::read_to_string(state_dir.join(PID_FILE))
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "allowroute is not running"))?;
    let pid: i32 = pid
        .trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid pid file"))?;
    // A stale pid file must not signal an unrelated recycled process. Check
    // /proc/<pid>/comm where it exists (Linux); elsewhere verify the pid is
    // at least alive — a dead pid is always stale.
    match fs::read_to_string(format!("/proc/{pid}/comm")) {
        Ok(comm) if comm.trim() != "allowroute" => {
            return Err(io::Error::new(io::ErrorKind::NotFound, "allowroute is not running"));
        }
        Ok(_) => {}
        Err(_) if unsafe { libc::kill(pid, 0) } != 0 => {
            return Err(io::Error::new(io::ErrorKind::NotFound, "allowroute is not running"));
        }
        Err(_) => {}
    }
    if unsafe { libc::kill(pid, libc::SIGUSR1) } != 0 {
        return Err(io::Error::last_os_error());
    }
    eprintln!("allowroute: clear requested");
    for tool in ["resolvectl", "systemd-resolve"] {
        if Command::new(tool)
            .arg("flush-caches")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            break;
        }
    }
    Ok(())
}

/// Removes nft redirect if `run` unwinds or returns early after `install_redirect` succeeded.
struct RedirectGuard {
    active: bool,
}

impl RedirectGuard {
    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for RedirectGuard {
    fn drop(&mut self) {
        if self.active {
            remove_redirect();
        }
    }
}

fn append_log(state_dir: &Path, line: &str) {
    let log_path = state_dir
        .parent()
        .unwrap_or(Path::new(RUNTIME_DIR))
        .join("allowroute.log");
    let _ = fs::create_dir_all(log_path.parent().unwrap_or(Path::new(RUNTIME_DIR)));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(file, "{line}");
    }
}

fn tcp_loop(listener: TcpListener, shared: Arc<Shared>) {
    while !STOP.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || {
                    if let Some(query) = read_tcp_dns(&mut stream) {
                        let reply = handle_query(&query, &shared);
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

fn handle_query(query: &[u8], shared: &Shared) -> Vec<u8> {
    let question = inspect_dns(query);
    let system = (shared.get_system_resolvers)(shared);
    let system_resp = shared.dns_io.query(query, &system);
    let system_addrs = system_resp.as_ref().and_then(|pkt| inspect_dns(pkt)).map(|view| view.answers).unwrap_or_default();
    let looked = decide(&system_addrs, &[]);
    if !looked.addresses.is_empty() && !looked.use_tunnel {
        // A name that fell back to the tunnel earlier but now resolves
        // publicly loses its tunnel route right away, not at TTL expiry.
        if let Some(view) = question {
            let key = RouteKey {
                name: view.qname,
                qtype: view.qtype,
            };
            let tracked = shared.route_state.lock().expect("route_state").by_name.contains_key(&key);
            if tracked {
                eprintln!("allowroute: {} resolves publicly again; removing tunnel route", key.name);
                remove_tracked(&key, shared);
            }
        }
        return system_resp.unwrap_or_else(|| servfail(query));
    }
    let tunnel_resp = shared.dns_io.query(query, &socket_addrs(&shared.tunnel_dns));
    let tunnel_addrs = tunnel_resp.as_ref().and_then(|pkt| inspect_dns(pkt)).map(|view| view.answers).unwrap_or_default();
    let looked = decide(&system_addrs, &tunnel_addrs);
    if looked.use_tunnel {
        let (name, qtype) = question.map(|v| (v.qname, v.qtype)).unwrap_or_default();
        install_answers(&name, qtype, &looked.addresses, shared);
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

fn forget(shared: &Shared, cidr: &str) {
    let mut guard = shared.routes.lock().expect("routes");
    if guard.iter().any(|have| have == cidr) {
        guard.retain(|have| have != cidr);
        let _ = write_routes(&shared.state_dir, &shared.tun, &guard);
    }
}

/// Route a tunnel answer through the tunnel and, when the answer came from a
/// named question, record it so the route expires after `route_ttl`. The
/// bookkeeping and the kernel changes share one critical section: a
/// concurrent `remove_tracked`/`clear_tracked` can never delete a route whose
/// refcount was just claimed, nor miss an install that already ran.
fn install_answers(name: &str, qtype: u16, addrs: &[IpAddr], shared: &Shared) {
    let mut state = shared.route_state.lock().expect("route_state");
    let mut installed = Vec::new();
    for addr in addrs {
        if shared.prefixes.iter().any(|prefix| prefix_covers(prefix, *addr)) {
            if !name.is_empty() {
                eprintln!("allowroute: {name} -> {addr} via tunnel (inside VPN prefix)");
            }
            continue;
        }
        let cidr = host_route(*addr);
        if shared.routes_io.add(&cidr).is_ok() {
            remember(shared, &cidr);
            installed.push(cidr);
            if !name.is_empty() {
                eprintln!("allowroute: {name} -> {addr} via tunnel");
            }
        }
    }
    if name.is_empty() {
        return;
    }
    let key = RouteKey { name: name.to_string(), qtype };
    let RouteState { by_name, refs } = &mut *state;
    let entry = by_name.entry(key).or_insert_with(|| TrackedRoute {
        cidrs: Vec::new(),
        expires: Instant::now(),
    });
    entry.expires = Instant::now() + shared.route_ttl;
    // Addresses that disappeared from the answer lose their route now, not
    // when the whole name is removed.
    let stale: Vec<String> = entry
        .cidrs
        .iter()
        .filter(|cidr| !installed.contains(cidr))
        .cloned()
        .collect();
    for cidr in stale {
        entry.cidrs.retain(|have| have != &cidr);
        release_cidr(refs, &cidr, shared);
    }
    for cidr in installed {
        if !entry.cidrs.contains(&cidr) {
            entry.cidrs.push(cidr.clone());
            *refs.entry(cidr).or_insert(0) += 1;
        }
    }
}

/// Drop every DNS-derived tunnel route. Kept for `clear`: names that fell
/// over to the tunnel re-decide on their next lookup instead of staying
/// pinned until disconnect.
fn clear_tracked(shared: &Shared) {
    let keys: Vec<RouteKey> = {
        let state = shared.route_state.lock().expect("route_state");
        state.by_name.keys().cloned().collect()
    };
    let count = keys.len();
    for key in keys {
        remove_tracked(&key, shared);
    }
    if count > 0 {
        eprintln!("allowroute: cleared {count} DNS-derived tunnel routes");
    }
}

/// Release one name's claim on a route; the last claim deletes the route.
/// Must be called with `route_state` held — see `install_answers`.
fn release_cidr(refs: &mut HashMap<String, usize>, cidr: &str, shared: &Shared) {
    match refs.get_mut(cidr) {
        Some(count) => {
            *count -= 1;
            if *count > 0 {
                return;
            }
        }
        None => {
            eprintln!("allowroute: refcount underflow for {cidr}");
            return;
        }
    }
    refs.remove(cidr);
    if shared.static_routes.contains(cidr) {
        // The CIDR is also a static route (prefix or tunnel-DNS host route):
        // the tracked name loses its claim but the route itself must stay.
        return;
    }
    shared.routes_io.del(cidr);
    forget(shared, cidr);
}

/// Remove a tracked name's host routes; a route shared with another tracked
/// name stays until that name also goes away.
fn remove_tracked(key: &RouteKey, shared: &Shared) {
    let mut state = shared.route_state.lock().expect("route_state");
    let RouteState { by_name, refs } = &mut *state;
    let Some(entry) = by_name.remove(key) else { return };
    for cidr in entry.cidrs {
        release_cidr(refs, &cidr, shared);
    }
}

/// Re-validate an expired tracked route: a name that now resolves publicly
/// loses its tunnel route; one that still only resolves through the tunnel is
/// renewed. When nothing resolves at all the route stays — the TTL does not
/// apply to an unresolvable name.
fn revalidate(key: &RouteKey, shared: &Shared) {
    let Some(query) = build_query(&key.name, key.qtype, QUERY_ID.fetch_add(1, Ordering::Relaxed)) else {
        remove_tracked(key, shared);
        return;
    };
    let system = (shared.get_system_resolvers)(shared);
    let system_addrs = shared
        .dns_io
        .query(&query, &system)
        .and_then(|pkt| inspect_dns(&pkt))
        .map(|view| view.answers)
        .unwrap_or_default();
    if system_addrs.iter().any(|ip| is_public_unicast(*ip)) {
        eprintln!("allowroute: {} now resolves publicly; removing tunnel route", key.name);
        remove_tracked(key, shared);
        return;
    }
    let tunnel_addrs = shared
        .dns_io
        .query(&query, &socket_addrs(&shared.tunnel_dns))
        .and_then(|pkt| inspect_dns(&pkt))
        .map(|view| view.answers)
        .unwrap_or_default();
    if !tunnel_addrs.is_empty() {
        install_answers(&key.name, key.qtype, &tunnel_addrs, shared);
        return;
    }
    let mut state = shared.route_state.lock().expect("route_state");
    if let Some(entry) = state.by_name.get_mut(key) {
        entry.expires = Instant::now() + shared.route_ttl;
    }
}

fn sweeper(shared: Arc<Shared>) {
    while !STOP.load(Ordering::Relaxed) {
        thread::sleep(SWEEP_INTERVAL);
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        let expired: Vec<RouteKey> = {
            let state = shared.route_state.lock().expect("route_state");
            state
                .by_name
                .iter()
                .filter(|(_, entry)| entry.expires <= Instant::now())
                .map(|(key, _)| key.clone())
                .collect()
        };
        for key in expired {
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            revalidate(&key, &shared);
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

fn system_resolvers(shared: &Shared) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for path in ["/run/systemd/resolve/resolv.conf", "/etc/resolv.conf"] {
        let Ok(text) = fs::read_to_string(path) else { continue };
        for addr in parse_nameservers(&text) {
            if is_loopback(addr) {
                continue;
            }
            if !underlay_route(addr, &shared.tun, &shared.tun_addrs) {
                let mut noted = shared.skipped_dns.lock().expect("skipped_dns");
                if noted.insert(addr) {
                    eprintln!("allowroute: ignoring nameserver {addr}: only reachable via {}", shared.tun);
                }
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

/// `nameserver` entries from a resolv.conf body, in order.
fn parse_nameservers(text: &str) -> impl Iterator<Item = IpAddr> + '_ {
    text.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next() != Some("nameserver") {
            return None;
        }
        parts.next()?.parse::<IpAddr>().ok()
    })
}

/// Addresses assigned to the tunnel interface itself; they can never answer
/// DNS because they are us.
fn iface_addrs(tun: &str) -> Vec<IpAddr> {
    let Ok(out) = Command::new(ip_bin()).args(["-o", "addr", "show", "dev", tun]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut addrs = Vec::new();
    let mut parts = text.split_whitespace();
    while let Some(token) = parts.next() {
        if token != "inet" && token != "inet6" {
            continue;
        }
        if let Some(addr) = parts
            .next()
            .and_then(|addr| addr.split('/').next())
            .and_then(|addr| addr.parse::<IpAddr>().ok())
        {
            addrs.push(addr);
        }
    }
    addrs
}

/// True when `ip` can be reached without the tunnel. A nameserver that only
/// exists behind the VPN — the tunnel's own address, or corporate DNS that
/// another tool wrote into resolv.conf — cannot serve the underlay and would
/// stall every query behind its timeout. Fail open when the route cannot be
/// determined: a wrong keep costs one timeout, a wrong drop loses a resolver.
fn underlay_route(ip: IpAddr, tun: &str, tun_addrs: &[IpAddr]) -> bool {
    let family = if ip.is_ipv4() { "-4" } else { "-6" };
    let Ok(out) = Command::new(ip_bin()).args([family, "route", "get", &ip.to_string()]).output() else {
        return true;
    };
    underlay_decision(out.status.success(), &String::from_utf8_lossy(&out.stdout), ip, tun, tun_addrs)
}

/// Whether a nameserver stays usable, decided from `ip route get` output.
/// `route_ok` false means the lookup itself failed — fail open. A resolver
/// routed via the tunnel is dropped (it cannot serve the underlay), and so
/// is the tunnel's own address when `route get` reports it via `dev lo`.
fn underlay_decision(route_ok: bool, route_text: &str, ip: IpAddr, tun: &str, tun_addrs: &[IpAddr]) -> bool {
    if !route_ok {
        return true;
    }
    if route_uses_dev(route_text, tun) {
        return false;
    }
    // The tunnel's own addresses resolve through the local table (`dev lo`);
    // other local addresses (a resolver bound to the WiFi address, say) stay.
    if route_uses_dev(route_text, "lo") && tun_addrs.contains(&ip) {
        return false;
    }
    true
}

/// True while the tunnel interface exists. Checked every loop pass so a
/// disconnect that never reaches vpnc-script still ends this daemon — the
/// DNS redirect must not outlive the tunnel.
#[cfg(target_os = "linux")]
fn iface_exists(name: &str) -> bool {
    Path::new("/sys/class/net").join(name).exists()
}

/// Non-Linux fallback; fail open so a failed check never kills a live daemon.
#[cfg(not(target_os = "linux"))]
fn iface_exists(name: &str) -> bool {
    Command::new(ip_bin())
        .args(["-o", "link", "show", "dev", name])
        .status()
        .map(|status| status.success())
        .unwrap_or(true)
}

/// True when the tunnel device vanished — the daemon is orphaned and only
/// harms name resolution until cleanup runs.
fn orphaned(shared: &Shared) -> bool {
    !(shared.iface_up)(&shared.tun)
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
    if fs::create_dir_all(RUNTIME_DIR).is_err() || fs::write(NFT_FILE, program).is_err() {
        return false;
    }
    let _ = Command::new(nft_bin()).args(["delete", "table", "inet", "gpallow"]).status();
    Command::new(nft_bin()).args(["-f", NFT_FILE]).status().map(|s| s.success()).unwrap_or(false)
}

fn remove_redirect() {
    let _ = Command::new(nft_bin()).args(["delete", "table", "inet", "gpallow"]).status();
    let _ = fs::remove_file(NFT_FILE);
}

fn cleanup(state_dir: &Path) -> io::Result<()> {
    remove_redirect();
    let _ = fs::remove_file(state_dir.join(PID_FILE));
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
        libc::signal(libc::SIGUSR1, on_clear as libc::sighandler_t);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn args(tokens: &[&str]) -> Args {
        parse_args(tokens.iter().map(|s| s.to_string()))
    }

    #[test]
    fn route_ttl_defaults_to_five_minutes() {
        assert_eq!(args(&[]).route_ttl, Duration::from_secs(300));
    }

    #[test]
    fn route_ttl_flag_and_equals_form() {
        assert_eq!(args(&["--route-ttl", "60"]).route_ttl, Duration::from_secs(60));
        assert_eq!(args(&["--route-ttl=120"]).route_ttl, Duration::from_secs(120));
        assert_eq!(args(&["--route-ttl", "0"]).route_ttl, Duration::ZERO);
    }

    #[test]
    fn route_ttl_does_not_swallow_the_next_flag() {
        let args = args(&["--route-ttl", "--prefix", "10.0.0.0/8"]);
        assert_eq!(args.route_ttl, Duration::from_secs(300));
        assert_eq!(args.prefixes, vec!["10.0.0.0/8".to_string()]);
    }

    #[test]
    fn route_ttl_rejects_garbage_and_clamps_huge() {
        assert_eq!(args(&["--route-ttl=abc"]).route_ttl, Duration::from_secs(300));
        assert_eq!(
            args(&["--route-ttl", "99999999999999"]).route_ttl,
            Duration::from_secs(MAX_ROUTE_TTL_SECS)
        );
    }

    // ---- Simulated connect/disconnect lifecycle ---------------------------
    //
    // These drive `handle_query`/`revalidate`/`clear_tracked` end to end with
    // fake route and DNS backends: no root, no sockets, no real resolver.

    const SYS_DNS: &str = "9.9.9.9";
    /// Tunnel DNS is *outside* the 10/8 prefix so it gets a static host route.
    const TUN_DNS: &str = "10.1.1.1";

    struct FakeRoutes {
        installed: Mutex<BTreeSet<String>>,
    }

    impl RouteCtl for FakeRoutes {
        fn add(&self, cidr: &str) -> io::Result<()> {
            self.installed.lock().expect("routes").insert(cidr.to_string());
            Ok(())
        }
        fn del(&self, cidr: &str) {
            self.installed.lock().expect("routes").remove(cidr);
        }
    }

    impl FakeRoutes {
        fn has(&self, cidr: &str) -> bool {
            self.installed.lock().expect("routes").contains(cidr)
        }
    }

    /// Reply packets per upstream server IP; `None` means the server times out.
    struct FakeDns {
        replies: Mutex<HashMap<IpAddr, Option<Vec<u8>>>>,
    }

    impl FakeDns {
        fn answer(&self, server: IpAddr, addrs: &[IpAddr]) {
            self.replies
                .lock()
                .expect("replies")
                .insert(server, Some(answer_packet(addrs)));
        }
        fn drop_server(&self, server: IpAddr) {
            self.replies.lock().expect("replies").insert(server, None);
        }
    }

    impl DnsIo for FakeDns {
        fn query(&self, _pkt: &[u8], servers: &[SocketAddr]) -> Option<Vec<u8>> {
            let replies = self.replies.lock().expect("replies");
            servers
                .iter()
                .filter_map(|s| replies.get(&s.ip()).cloned().flatten())
                .next()
        }
    }

    /// A DNS response whose question is `x` and whose answers are `addrs`.
    fn answer_packet(addrs: &[IpAddr]) -> Vec<u8> {
        let mut pkt = vec![0u8, 0, 0x81, 0x80, 0, 1, 0, addrs.len() as u8, 0, 0, 0, 0];
        pkt.extend_from_slice(&[1, b'x', 0, 0, 1, 0, 1]);
        for addr in addrs {
            let (rtype, rdata) = match addr {
                IpAddr::V4(v4) => (1u16, v4.octets().to_vec()),
                IpAddr::V6(v6) => (28u16, v6.octets().to_vec()),
            };
            pkt.extend_from_slice(&[0xc0, 0x0c]);
            pkt.extend_from_slice(&rtype.to_be_bytes());
            pkt.extend_from_slice(&1u16.to_be_bytes());
            pkt.extend_from_slice(&60u32.to_be_bytes());
            pkt.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            pkt.extend_from_slice(&rdata);
        }
        pkt
    }

    struct Fixture {
        shared: Arc<Shared>,
        routes: Arc<FakeRoutes>,
        dns: Arc<FakeDns>,
        tun_up: Arc<AtomicBool>,
        /// Resolver IPs the injected `system_resolvers` returns; empty
        /// simulates "every nameserver routes via the tunnel".
        system_servers: Arc<Mutex<Vec<IpAddr>>>,
        sys: IpAddr,
        tun_dns: IpAddr,
    }

    static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn fixture(static_routes: &[&str]) -> Fixture {
        let routes = Arc::new(FakeRoutes { installed: Mutex::new(BTreeSet::new()) });
        let dns = Arc::new(FakeDns { replies: Mutex::new(HashMap::new()) });
        let tun_up = Arc::new(AtomicBool::new(true));
        let sys: IpAddr = SYS_DNS.parse().unwrap();
        let tun_dns: IpAddr = TUN_DNS.parse().unwrap();
        let system_servers = Arc::new(Mutex::new(vec![sys]));
        let dir = std::env::temp_dir().join(format!(
            "allowroute-test-{}-{}",
            std::process::id(),
            TEST_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let shared = Arc::new(Shared {
            tun: "tun0".to_string(),
            tun_addrs: Vec::new(),
            state_dir: dir,
            prefixes: vec!["10.0.0.0/8".to_string()],
            tunnel_dns: vec![tun_dns],
            route_ttl: Duration::from_secs(300),
            static_routes: static_routes.iter().map(|s| s.to_string()).collect(),
            routes: Mutex::new(Vec::new()),
            route_state: Mutex::new(RouteState::default()),
            skipped_dns: Mutex::new(HashSet::new()),
            routes_io: Arc::clone(&routes) as Arc<dyn RouteCtl>,
            dns_io: Arc::clone(&dns) as Arc<dyn DnsIo>,
            get_system_resolvers: Arc::new({
                let system_servers = Arc::clone(&system_servers);
                move |_| {
                    system_servers
                        .lock()
                        .expect("system_servers")
                        .iter()
                        .map(|ip| SocketAddr::new(*ip, 53))
                        .collect()
                }
            }),
            iface_up: Arc::new({
                let tun_up = Arc::clone(&tun_up);
                move |_| tun_up.load(Ordering::Relaxed)
            }),
        });
        Fixture { shared, routes, dns, tun_up, system_servers, sys, tun_dns }
    }

    fn key(name: &str, qtype: u16) -> RouteKey {
        RouteKey { name: name.to_string(), qtype }
    }

    fn tracked_entry(shared: &Shared, name: &str, qtype: u16) -> Option<(Vec<String>, Instant)> {
        let state = shared.route_state.lock().expect("route_state");
        state.by_name.get(&key(name, qtype)).map(|e| (e.cidrs.clone(), e.expires))
    }

    fn expire_entry(shared: &Shared, name: &str, qtype: u16) {
        let mut state = shared.route_state.lock().expect("route_state");
        if let Some(entry) = state.by_name.get_mut(&key(name, qtype)) {
            entry.expires = Instant::now() - Duration::from_secs(1);
        }
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// Full fallback path: no system answer, tunnel answers a private address
    /// outside the VPN prefix -> host route installed, name tracked.
    #[test]
    fn tunnel_fallback_installs_and_tracks_route() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();

        let reply = handle_query(&q, &fx.shared);

        assert_eq!(inspect_dns(&reply).unwrap().answers, vec![ip("172.16.9.10")]);
        assert!(fx.routes.has("172.16.9.10/32"));
        let (cidrs, _) = tracked_entry(&fx.shared, "corp.internal", 1).expect("tracked");
        assert_eq!(cidrs, vec!["172.16.9.10/32".to_string()]);
    }

    /// A name that fell back earlier now resolves publicly -> the tunnel route
    /// is removed on the very next query, not at TTL expiry.
    #[test]
    fn public_answer_later_removes_tunnel_route() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();
        handle_query(&q, &fx.shared);
        assert!(fx.routes.has("172.16.9.10/32"));

        fx.dns.answer(fx.sys, &[ip("93.184.216.34")]);
        let reply = handle_query(&q, &fx.shared);

        assert_eq!(inspect_dns(&reply).unwrap().answers, vec![ip("93.184.216.34")]);
        assert!(!fx.routes.has("172.16.9.10/32"));
        assert!(tracked_entry(&fx.shared, "corp.internal", 1).is_none());
    }

    /// Sweeper revalidation: expired entry + public system answer -> removed.
    #[test]
    fn revalidate_removes_name_that_now_resolves_publicly() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();
        handle_query(&q, &fx.shared);
        expire_entry(&fx.shared, "corp.internal", 1);

        fx.dns.answer(fx.sys, &[ip("93.184.216.34")]);
        revalidate(&key("corp.internal", 1), &fx.shared);

        assert!(!fx.routes.has("172.16.9.10/32"));
        assert!(tracked_entry(&fx.shared, "corp.internal", 1).is_none());
    }

    /// Both resolvers dead -> the route is kept and TTL renewed, so a resolver
    /// outage can never strand a name behind a stale route.
    #[test]
    fn unresolvable_name_keeps_route_and_renews_ttl() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();
        handle_query(&q, &fx.shared);
        expire_entry(&fx.shared, "corp.internal", 1);

        fx.dns.drop_server(fx.tun_dns);
        revalidate(&key("corp.internal", 1), &fx.shared);

        let (_, expires) = tracked_entry(&fx.shared, "corp.internal", 1).expect("still tracked");
        assert!(expires > Instant::now());
        assert!(fx.routes.has("172.16.9.10/32"));
    }

    /// A fresh tunnel answer drops CIDRs the answer no longer carries.
    #[test]
    fn fresh_tunnel_answer_replaces_stale_cidrs() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10"), ip("172.16.9.11")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();
        handle_query(&q, &fx.shared);
        expire_entry(&fx.shared, "corp.internal", 1);

        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.11")]);
        revalidate(&key("corp.internal", 1), &fx.shared);

        assert!(!fx.routes.has("172.16.9.10/32"));
        assert!(fx.routes.has("172.16.9.11/32"));
        let (cidrs, _) = tracked_entry(&fx.shared, "corp.internal", 1).expect("tracked");
        assert_eq!(cidrs, vec!["172.16.9.11/32".to_string()]);
    }

    /// Answers inside a VPN prefix install no host route but still renew TTL —
    /// regression for the every-5s revalidation churn.
    #[test]
    fn prefix_covered_answers_renew_ttl_without_route() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("10.9.9.9")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();
        handle_query(&q, &fx.shared);

        assert!(!fx.routes.has("10.9.9.9/32"));
        let (cidrs, first_expiry) = tracked_entry(&fx.shared, "corp.internal", 1).expect("tracked");
        assert!(cidrs.is_empty());

        expire_entry(&fx.shared, "corp.internal", 1);
        revalidate(&key("corp.internal", 1), &fx.shared);
        let (_, renewed) = tracked_entry(&fx.shared, "corp.internal", 1).expect("tracked");
        assert!(renewed > first_expiry || renewed > Instant::now());
    }

    #[test]
    fn clear_tracked_removes_every_dns_derived_route() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        for name in ["a.internal", "b.internal"] {
            let q = build_query(name, 1, 0x1234).unwrap();
            handle_query(&q, &fx.shared);
        }
        assert!(fx.routes.has("172.16.9.10/32"));

        clear_tracked(&fx.shared);

        assert!(!fx.routes.has("172.16.9.10/32"));
        assert!(fx.shared.route_state.lock().expect("route_state").by_name.is_empty());
    }

    /// A CIDR shared by two names stays until the last claim is released.
    #[test]
    fn shared_cidr_survives_until_last_name_removed() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        for name in ["a.internal", "b.internal"] {
            let q = build_query(name, 1, 0x1234).unwrap();
            handle_query(&q, &fx.shared);
        }

        remove_tracked(&key("a.internal", 1), &fx.shared);
        assert!(fx.routes.has("172.16.9.10/32"));

        remove_tracked(&key("b.internal", 1), &fx.shared);
        assert!(!fx.routes.has("172.16.9.10/32"));
    }

    /// Regression: a tracked name resolving to the tunnel-DNS address (a
    /// static route) must not delete that route when released — losing it
    /// breaks every later tunnel query.
    #[test]
    fn static_route_survives_tracked_release() {
        let tun_cidr = "10.1.1.1/32";
        let fx = fixture(&[tun_cidr]);
        fx.routes.installed.lock().expect("routes").insert(tun_cidr.to_string());
        fx.shared.routes.lock().expect("routes").push(tun_cidr.to_string());
        fx.dns.drop_server(fx.sys);
        fx.dns.answer(fx.tun_dns, &[fx.tun_dns]);
        let q = build_query("dns.corp.internal", 1, 0x1234).unwrap();
        handle_query(&q, &fx.shared);

        remove_tracked(&key("dns.corp.internal", 1), &fx.shared);

        assert!(fx.routes.has(tun_cidr));
        assert!(fx.shared.routes.lock().expect("routes").contains(&tun_cidr.to_string()));
    }

    /// Blackout reproduction: when every system resolver is skipped (all
    /// nameservers route via the tunnel, e.g. broad split-includes), queries
    /// fall to the tunnel alone — the observed dead-DNS state once the tunnel
    /// dies behind a live daemon.
    #[test]
    fn empty_system_list_sends_everything_to_tunnel() {
        let fx = fixture(&[]);
        fx.system_servers.lock().expect("system_servers").clear();
        fx.dns.answer(fx.tun_dns, &[ip("172.16.9.10")]);
        let q = build_query("corp.internal", 1, 0x1234).unwrap();

        let reply = handle_query(&q, &fx.shared);

        assert_eq!(inspect_dns(&reply).unwrap().answers, vec![ip("172.16.9.10")]);
        assert!(fx.routes.has("172.16.9.10/32"));
    }

    /// Orphan watchdog: the daemon must want to stop once the tunnel device
    /// vanishes (disconnect path that never reached vpnc-script).
    #[test]
    fn orphan_watchdog_trips_when_tun_vanishes() {
        let fx = fixture(&[]);
        assert!(!orphaned(&fx.shared));
        fx.tun_up.store(false, Ordering::Relaxed);
        assert!(orphaned(&fx.shared));
    }

    #[test]
    fn nothing_resolves_returns_servfail() {
        let fx = fixture(&[]);
        fx.dns.drop_server(fx.sys);
        fx.dns.drop_server(fx.tun_dns);
        let q = build_query("gone.internal", 1, 0x1234).unwrap();
        let reply = handle_query(&q, &fx.shared);
        assert_eq!(&reply[2..4], &[0x81, 0x82]);
    }

    #[test]
    fn parse_nameservers_reads_only_nameserver_lines() {
        let text = "# comment\nnameserver 8.8.8.8\nsearch corp.local\nnameserver 1.1.1.1 extra\nnameserver bogus\n";
        let found: Vec<IpAddr> = parse_nameservers(text).collect();
        assert_eq!(found, vec![ip("8.8.8.8"), ip("1.1.1.1")]);
    }

    #[test]
    fn underlay_decision_skips_tunnel_only_resolvers() {
        let tun_addrs = vec![ip("10.4.0.2")];
        // Resolver reachable via the underlay stays.
        assert!(underlay_decision(true, "8.8.8.8 via 192.168.1.1 dev wlan0", ip("8.8.8.8"), "tun0", &tun_addrs));
        // Resolver only reachable via the tunnel is dropped.
        assert!(!underlay_decision(true, "10.20.0.1 dev tun0 scope link", ip("10.20.0.1"), "tun0", &tun_addrs));
        // The tunnel's own address (dev lo) is dropped.
        assert!(!underlay_decision(true, "local 10.4.0.2 dev lo", ip("10.4.0.2"), "tun0", &tun_addrs));
        // Route lookup failure fails open: keep the resolver.
        assert!(underlay_decision(false, "", ip("8.8.8.8"), "tun0", &tun_addrs));
    }
}
