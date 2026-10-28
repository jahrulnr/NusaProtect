use std::{sync::Arc, time::Duration};

use gpapi::{
  credential::{Credential, PasswordCredential},
  gateway::{Gateway, GatewayLogin, GatewayLoginClient},
  gp_params::GpParams,
  hip::HipSource,
  os_profile::{ClientOs, OsProfile},
  portal::{PortalConfig, Prelogin, PreloginOptions, prelogin, retrieve_config},
  process::auth_launcher::SamlAuthLauncher,
  service::{request::ConnectRequest, vpn_state::ConnectInfo},
  session::{AuthenticatedGateway, ConnectionPlan},
};
use log::info;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

/// Resolves the gateway to connect to; asks the user via a `choose-gateway`
/// event when the portal offers more than one.
pub struct GatewayPick {
  pending: Arc<Mutex<Option<oneshot::Sender<String>>>>,
}

impl GatewayPick {
  pub fn new(pending: Arc<Mutex<Option<oneshot::Sender<String>>>>) -> Self {
    Self { pending }
  }

  /// `choice` is a preferred gateway name; falls back to the interactive picker.
  async fn pick(
    &self,
    choice: Option<&str>,
    portal_config: &PortalConfig,
    gateways: &[&Gateway],
    app: &AppHandle,
  ) -> anyhow::Result<Gateway> {
    if let Some(g) = choice {
      if let Some(gw) = portal_config.find_gateway(g) {
        return Ok(gw.clone());
      }
    }
    if gateways.len() == 1 {
      return Ok(gateways[0].clone());
    }

    let list: Vec<Gateway> = gateways.iter().map(|g| (*g).clone()).collect();
    let (tx, rx) = oneshot::channel();
    *self.pending.lock().await = Some(tx);
    let _ = app.emit("choose-gateway", &list);
    let result = tokio::time::timeout(Duration::from_secs(300), rx).await;
    *self.pending.lock().await = None;

    match result {
      // Empty string = "auto" → first gateway in priority order
      Ok(Ok(name)) if name.is_empty() => Ok(gateways[0].clone()),
      Ok(Ok(name)) => portal_config
        .find_gateway(&name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Gateway not found: {}", name)),
      _ => anyhow::bail!("Gateway selection cancelled"),
    }
  }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PreloginInfo {
  Saml,
  Standard {
    auth_message: String,
    label_username: String,
    label_password: String,
  },
}

#[derive(Debug, Clone)]
pub struct CertConfig {
  pub certificate: Option<String>,
  pub sslkey: Option<String>,
  pub key_password: Option<String>,
}

/// OS spoof profile from settings; empty strings = use OS defaults.
#[derive(Debug, Clone)]
pub struct OsProfileConfig {
  pub client_os: Option<String>,
  pub os_version: Option<String>,
  pub client_version: Option<String>,
  pub user_agent: Option<String>,
}

impl OsProfileConfig {
  fn to_os_profile(&self) -> OsProfile {
    let client_os = match self.client_os.as_deref() {
      Some("windows") => ClientOs::Windows,
      Some("macos") => ClientOs::Mac,
      _ => ClientOs::Linux,
    };
    let mut builder = OsProfile::builder(client_os);
    if let Some(v) = self.os_version.as_deref().filter(|v| !v.is_empty()) {
      builder = builder.os_version(v);
    }
    if let Some(v) = self.user_agent.as_deref().filter(|v| !v.is_empty()) {
      builder = builder.user_agent(v);
    }
    // client_version is not filtered by the builder — guard empty here.
    if let Some(v) = self.client_version.as_deref().filter(|v| !v.trim().is_empty()) {
      builder = builder.client_version(v);
    }
    builder.build()
  }
}

#[derive(Debug, Clone)]
pub struct TunnelConfig {
  pub disable_ipv6: bool,
  pub no_dtls: bool,
  pub vpnc_script: Option<String>,
  pub reconnect_timeout: u32,
  pub mtu: u32,
  pub hip_source: HipSource,
}

/// Auth artifacts kept after a successful connect so the user can switch
/// gateways without going through authentication again.
#[derive(Clone)]
pub struct CachedAuth {
  pub portal: String,
  pub region: String,
  pub cred: Credential,
  pub cert: CertConfig,
  pub tunnel: TunnelConfig,
  pub profile: OsProfileConfig,
}

/// Settings-derived options shared by the SAML and password connect flows.
pub struct ConnectOpts {
  pub cert: CertConfig,
  pub tunnel: TunnelConfig,
  pub profile: OsProfileConfig,
}

/// Browser/auth-launcher knobs for the SAML flow.
pub struct SamlOpts {
  pub browser: Option<String>,
  pub auth_executable: Option<String>,
  pub clean: bool,
  pub default_browser: bool,
  pub hidpi: bool,
}

/// Per-invocation flow plumbing: app handle, gateway picker, cancellation.
pub struct FlowCtx<'a> {
  pub app: &'a AppHandle,
  pub pick: GatewayPick,
  pub cancellation: &'a CancellationToken,
}

fn build_gp_params(cert: &CertConfig, profile_cfg: &OsProfileConfig) -> GpParams {
  let profile = profile_cfg.to_os_profile();
  let mut builder = GpParams::builder(profile);
  builder
    .certificate(cert.certificate.clone())
    .sslkey(cert.sslkey.clone())
    .key_password(cert.key_password.clone());
  builder.build()
}

pub async fn get_prelogin(portal: &str, cert: CertConfig, profile: OsProfileConfig) -> anyhow::Result<PreloginInfo> {
  let gp_params = build_gp_params(&cert, &profile);
  let result = prelogin(portal, &gp_params, PreloginOptions::default()).await?;

  let info = match result {
    Prelogin::Saml(_) => PreloginInfo::Saml,
    Prelogin::Standard(p) => PreloginInfo::Standard {
      auth_message: p.auth_message().to_owned(),
      label_username: p.label_username().to_owned(),
      label_password: p.label_password().to_owned(),
    },
  };

  Ok(info)
}

async fn gateway_auth(
  gateway: &str,
  cred: &Credential,
  gp_params: GpParams,
  cancellation: &CancellationToken,
) -> anyhow::Result<gpapi::session::GatewayAuthentication> {
  let mut client = GatewayLoginClient::new(gateway, gp_params)?;
  match client.login(cred, None, cancellation).await? {
    GatewayLogin::Authenticated(authentication) => Ok(authentication),
    GatewayLogin::Mfa(_, _) => {
      anyhow::bail!("MFA is not supported in the GUI yet. Please use the CLI: gpclient connect")
    }
  }
}

/// Full connect flow for SAML (browser-based) authentication.
/// Returns the ConnectRequest plus the CachedAuth kept for gateway switching.
pub async fn connect_saml(
  portal: &str,
  gateway: Option<&str>,
  saml: SamlOpts,
  opts: ConnectOpts,
  ctx: FlowCtx<'_>,
) -> anyhow::Result<(ConnectRequest, CachedAuth)> {
  let SamlOpts { browser, auth_executable, clean, default_browser, hidpi } = saml;
  let ConnectOpts { cert, tunnel, profile } = opts;
  let FlowCtx { app, pick, cancellation } = ctx;
  let gp_params = build_gp_params(&cert, &profile);
  let os_profile = gp_params.os_profile().clone();

  info!("Running prelogin for portal: {}", portal);
  let options = PreloginOptions::default().external_browser_requested(default_browser);
  let portal_prelogin = prelogin(portal, &gp_params, options).await?;
  let region = portal_prelogin.region().to_owned();

  let saml_request = match &portal_prelogin {
    Prelogin::Saml(p) => Some(p.saml_request().to_owned()),
    Prelogin::Standard(_) => None,
  };

  info!("Launching browser authentication (clean={}, default_browser={})", clean, default_browser);
  let mut launcher = SamlAuthLauncher::new(portal)
    .auth_executable(auth_executable.as_deref())
    .os_profile(&os_profile)
    .certificate(cert.certificate.as_deref())
    .sslkey(cert.sslkey.as_deref())
    .key_password(cert.key_password.as_deref())
    .hidpi(hidpi)
    .browser(browser.as_deref())
    .clean(clean)
    .default_browser(default_browser && browser.is_none())
    .cancellation(cancellation);
  if let Some(ref req) = saml_request {
    launcher = launcher.saml_request(req);
  }
  let cred = launcher.launch().await?;

  info!("Retrieving portal configuration");
  let mut portal_config = retrieve_config(portal, &cred, &gp_params, cancellation).await?;
  portal_config.sort_gateways(&region);

  let gateways = portal_config.gateways();
  anyhow::ensure!(!gateways.is_empty(), "No gateways found in portal configuration");

  let selected_gateway = pick.pick(gateway, &portal_config, &gateways, app).await?;
  let all_gateways: Vec<_> = gateways.into_iter().cloned().collect();

  info!("Connecting to gateway: {}", selected_gateway.name());

  let auth_cookie_cred: Credential = portal_config.auth_cookie().into();
  let authentication = match gateway_auth(selected_gateway.server(), &auth_cookie_cred, gp_params.clone(), cancellation).await {
    Ok(authentication) => authentication,
    Err(e) => {
      // Portal auth cookie rejected — fall back to gateway-level auth
      info!(
        "Portal auth cookie rejected by gateway ({}), trying gateway-level authentication",
        e
      );
      let mut gw_params = gp_params.clone();
      gw_params.set_is_gateway(true);

      let gw_prelogin = prelogin(selected_gateway.server(), &gw_params, options).await?;
      let gw_cred = match &gw_prelogin {
        Prelogin::Saml(saml) => {
          info!("Launching gateway browser authentication");
          let saml_req = saml.saml_request().to_owned();
          SamlAuthLauncher::new(selected_gateway.server())
            .auth_executable(auth_executable.as_deref())
            .os_profile(&os_profile)
            .certificate(cert.certificate.as_deref())
            .sslkey(cert.sslkey.as_deref())
            .key_password(cert.key_password.as_deref())
            .hidpi(hidpi)
            .browser(browser.as_deref())
            .clean(clean)
            .default_browser(default_browser && browser.is_none())
            .gateway(true)
            .saml_request(&saml_req)
            .cancellation(cancellation)
            .launch()
            .await?
        }
        Prelogin::Standard(_) => {
          anyhow::bail!(
            "Gateway requires username/password authentication. Please use the CLI: gpclient connect"
          )
        }
      };

      gateway_auth(selected_gateway.server(), &gw_cred, gw_params, cancellation).await?
    }
  };

  let policy = portal_config.internal_session_policy().unwrap_or_default();
  let plan = ConnectionPlan::new(
    vec![AuthenticatedGateway {
      gateway: selected_gateway.clone(),
      authentication,
      binding: None,
    }],
    vec![],
    policy,
  )?;

  let connect_info = ConnectInfo::new(portal.to_owned(), selected_gateway, all_gateways);
  let req = ConnectRequest::new(connect_info, plan)
    .with_os_profile(&os_profile)
    .with_disable_ipv6(tunnel.disable_ipv6)
    .with_no_dtls(tunnel.no_dtls)
    .with_vpnc_script(tunnel.vpnc_script.clone())
    .with_reconnect_timeout(tunnel.reconnect_timeout)
    .with_mtu(tunnel.mtu)
    .with_hip_source(tunnel.hip_source.clone())
    .with_certificate(cert.certificate.clone())
    .with_sslkey(cert.sslkey.clone())
    .with_key_password(cert.key_password.clone());
  let cached = CachedAuth { portal: portal.to_owned(), region, cred: auth_cookie_cred.clone(), cert, tunnel, profile };
  Ok((req, cached))
}

/// Full connect flow for standard (username/password) authentication.
/// Returns the ConnectRequest plus the CachedAuth kept for gateway switching.
pub async fn connect_password(
  portal: &str,
  gateway: Option<&str>,
  username: &str,
  password: &str,
  opts: ConnectOpts,
  ctx: FlowCtx<'_>,
) -> anyhow::Result<(ConnectRequest, CachedAuth)> {
  let ConnectOpts { cert, tunnel, profile } = opts;
  let FlowCtx { app, pick, cancellation } = ctx;
  let gp_params = build_gp_params(&cert, &profile);
  let os_profile = gp_params.os_profile().clone();

  info!("Running prelogin for portal: {}", portal);
  let prelogin_result = prelogin(portal, &gp_params, PreloginOptions::default()).await?;
  let region = prelogin_result.region().to_owned();

  let cred = Credential::Password(PasswordCredential::new(username, password));

  info!("Retrieving portal configuration");
  let mut portal_config = retrieve_config(portal, &cred, &gp_params, cancellation).await?;
  portal_config.sort_gateways(&region);

  let gateways = portal_config.gateways();
  anyhow::ensure!(!gateways.is_empty(), "No gateways found in portal configuration");

  let selected_gateway = pick.pick(gateway, &portal_config, &gateways, app).await?;
  let all_gateways: Vec<_> = gateways.into_iter().cloned().collect();

  info!("Connecting to gateway: {}", selected_gateway.name());

  let auth_cookie_cred: Credential = portal_config.auth_cookie().into();
  let authentication =
    gateway_auth(selected_gateway.server(), &auth_cookie_cred, gp_params, cancellation).await?;

  let policy = portal_config.internal_session_policy().unwrap_or_default();
  let plan = ConnectionPlan::new(
    vec![AuthenticatedGateway {
      gateway: selected_gateway.clone(),
      authentication,
      binding: None,
    }],
    vec![],
    policy,
  )?;

  let connect_info = ConnectInfo::new(portal.to_owned(), selected_gateway, all_gateways);
  let req = ConnectRequest::new(connect_info, plan)
    .with_os_profile(&os_profile)
    .with_disable_ipv6(tunnel.disable_ipv6)
    .with_no_dtls(tunnel.no_dtls)
    .with_vpnc_script(tunnel.vpnc_script.clone())
    .with_reconnect_timeout(tunnel.reconnect_timeout)
    .with_mtu(tunnel.mtu)
    .with_hip_source(tunnel.hip_source.clone())
    .with_certificate(cert.certificate.clone())
    .with_sslkey(cert.sslkey.clone())
    .with_key_password(cert.key_password.clone());
  let cached = CachedAuth { portal: portal.to_owned(), region, cred: auth_cookie_cred.clone(), cert, tunnel, profile };
  Ok((req, cached))
}

/// Switch to another gateway reusing the cached portal session — no re-auth.
/// Fails if the cached cookie is expired/rejected; caller should fall back to
/// a normal connect in that case.
pub async fn connect_with_credential(
  cached: &CachedAuth,
  gateway: &str,
  ctx: FlowCtx<'_>,
) -> anyhow::Result<ConnectRequest> {
  let FlowCtx { app, pick, cancellation } = ctx;
  let gp_params = build_gp_params(&cached.cert, &cached.profile);
  let os_profile = gp_params.os_profile().clone();

  let mut portal_config = retrieve_config(&cached.portal, &cached.cred, &gp_params, cancellation).await?;
  portal_config.sort_gateways(&cached.region);

  let gateways = portal_config.gateways();
  anyhow::ensure!(!gateways.is_empty(), "No gateways found in portal configuration");

  let selected_gateway = pick.pick(Some(gateway), &portal_config, &gateways, app).await?;
  let all_gateways: Vec<_> = gateways.into_iter().cloned().collect();

  info!("Switching to gateway: {}", selected_gateway.name());

  let auth_cookie_cred: Credential = portal_config.auth_cookie().into();
  let authentication = gateway_auth(selected_gateway.server(), &auth_cookie_cred, gp_params.clone(), cancellation)
    .await
    .map_err(|e| anyhow::anyhow!("Re-auth with cached session failed ({}). Connect again to re-authenticate.", e))?;

  let policy = portal_config.internal_session_policy().unwrap_or_default();
  let plan = ConnectionPlan::new(
    vec![AuthenticatedGateway {
      gateway: selected_gateway.clone(),
      authentication,
      binding: None,
    }],
    vec![],
    policy,
  )?;

  let connect_info = ConnectInfo::new(cached.portal.clone(), selected_gateway, all_gateways);
  Ok(
    ConnectRequest::new(connect_info, plan)
      .with_os_profile(&os_profile)
      .with_disable_ipv6(cached.tunnel.disable_ipv6)
      .with_no_dtls(cached.tunnel.no_dtls)
      .with_vpnc_script(cached.tunnel.vpnc_script.clone())
      .with_reconnect_timeout(cached.tunnel.reconnect_timeout)
      .with_mtu(cached.tunnel.mtu)
      .with_hip_source(cached.tunnel.hip_source.clone())
      .with_certificate(cached.cert.certificate.clone())
      .with_sslkey(cached.cert.sslkey.clone())
      .with_key_password(cached.cert.key_password.clone()),
  )
}
