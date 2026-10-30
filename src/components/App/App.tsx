import { useEffect, useState, useCallback, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import {
  CssBaseline,
  ThemeProvider,
  Box,
  Alert,
  Button,
  Snackbar,
  IconButton,
  Menu,
  MenuItem,
  ListItemIcon,
  ListItemText,
  Typography,
} from "@mui/material";
import MenuIcon from "@mui/icons-material/Menu";
import GitHubIcon from "@mui/icons-material/GitHub";
import RemoveIcon from "@mui/icons-material/Remove";
import CloseIcon from "@mui/icons-material/Close";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import SettingsIcon from "@mui/icons-material/Settings";
import PowerSettingsNewIcon from "@mui/icons-material/PowerSettingsNew";
import { VpnState, PreloginType, Gateway, TunnelState } from "../../types";
import DisconnectedView from "../DisconnectedView";
import ConnectingView from "../ConnectingView";
import ConnectedView from "../ConnectedView";
import DisconnectingView from "../DisconnectingView";
import { useAppTheme } from "../../useAppTheme";

function getStateTag(vpnState: VpnState | null): string {
  if (!vpnState) return "disconnected";
  if (vpnState === "disconnected") return "disconnected";
  if (vpnState === "disconnecting") return "disconnecting";
  if ("connecting" in vpnState) return "connecting";
  if ("connected" in vpnState) return "connected";
  if ("reconnectRequired" in vpnState) return "connecting";
  return "disconnected";
}

const PORTAL_KEY = "nusaprotect_last_portal";
const GATEWAY_KEY = "nusaprotect_gateway";
const GATEWAYS_CACHE_KEY = "nusaprotect_gateways";

type StoredSettings = {
  clientOs?: string;
  osVersion?: string;
  clientVersion?: string;
  userAgent?: string;
  vpncScript?: string;
  vpnPrefixes?: string;
  reconnectTimeout?: string;
  mtu?: string;
  submitHipReport?: boolean;
  hipScriptPath?: string;
};

// OS spoof profile args shared by get_prelogin/connect_* invokes.
const profileArgs = (s: StoredSettings) => ({
  clientOs: s.clientOs ?? null,
  osVersion: s.osVersion?.trim() || null,
  clientVersion: s.clientVersion?.trim() || null,
  userAgent: s.userAgent?.trim() || null,
});

// Tunnel args for connect_saml/connect_password.
const numArg = (v?: string): number | null => {
  const n = v ? parseInt(v, 10) : NaN;
  return Number.isFinite(n) && n > 0 ? n : null;
};
const tunnelArgs = (s: StoredSettings) => ({
  vpncScript: s.vpncScript?.trim() || null,
  vpnPrefixes: s.vpnPrefixes?.trim() || "10.0.0.0/8",
  reconnectTimeout: numArg(s.reconnectTimeout),
  mtu: numArg(s.mtu),
  submitHipReport: s.submitHipReport ?? true,
  hipScriptPath: s.hipScriptPath?.trim() || null,
});

export default function App() {
  const theme = useAppTheme();
  const [vpnState, setVpnState] = useState<VpnState | null>(null);
  const [connectedAt, setConnectedAt] = useState<number>(0);
  const [portal, setPortal] = useState(() => localStorage.getItem(PORTAL_KEY) ?? "");
  const [gateway, setGateway] = useState(() => localStorage.getItem(GATEWAY_KEY) ?? "");
  const [gateways, setGateways] = useState(() => {
    try {
      return JSON.parse(localStorage.getItem(GATEWAYS_CACHE_KEY) ?? "[]");
    } catch {
      return [];
    }
  });
  const [preloginInfo, setPreloginInfo] = useState<PreloginType | null>(null);
  const [isAuthenticating, setIsAuthenticating] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [gatewayChoices, setGatewayChoices] = useState<Gateway[] | null>(null);
  const [serviceError, setServiceError] = useState<string | null>(null);
  const [version, setVersion] = useState("");
  const [menuAnchor, setMenuAnchor] = useState<null | HTMLElement>(null);
  const connectInFlightRef = useRef(false);
  const connectCancelledRef = useRef(false);
  const vpnStateRef = useRef<VpnState | null>(null);
  const autoConnectAttemptedRef = useRef(false);
  const reconnectAttemptedRef = useRef<string | null>(null);
  const [tunnelState, setTunnelState] = useState<TunnelState | null>(null);

  useEffect(() => {
    if (portal) localStorage.setItem(PORTAL_KEY, portal);
    else localStorage.removeItem(PORTAL_KEY);
  }, [portal]);

  useEffect(() => {
    if (gateway) localStorage.setItem(GATEWAY_KEY, gateway);
    else localStorage.removeItem(GATEWAY_KEY);
  }, [gateway]);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});

    const showWindow = async () => {
      try {
        await invoke("show_window");
      } catch {
        // ignore
      }
    };

    // If an auto-connect can't proceed while the state is tagged "connecting"
    // (e.g. a reconnectRequired the GUI can't fulfill), drop back to the
    // disconnected view so the connect form stays reachable.
    const unstick = () => {
      if (getStateTag(vpnStateRef.current) === "connecting") {
        setVpnState("disconnected");
        vpnStateRef.current = "disconnected";
      }
    };

    // Shared auto-connect routine: cookie-reuse SAML connect without user interaction.
    // Falls back to showing the window (when showWindowOnFallback) if it can't proceed.
    const attemptAutoConnect = async (showWindowOnFallback: boolean) => {
      const s = JSON.parse(localStorage.getItem("nusaprotect_settings") ?? "{}");
      const savedPortal = localStorage.getItem(PORTAL_KEY);

      if (!savedPortal || !(s.reuseAuthCookies ?? true)) {
        unstick();
        if (showWindowOnFallback) await showWindow();
        return;
      }

      if (connectInFlightRef.current) {
        if (showWindowOnFallback) await showWindow();
        return;
      }

      connectInFlightRef.current = true;
      connectCancelledRef.current = false;
      setIsAuthenticating(true);
      setConnectError(null);
      try {
        const prelogin = await invoke<PreloginType>("get_prelogin", {
          portal: savedPortal,
          certificate: s.useClientCertificate ? (s.clientCertificate ?? null) : null,
          sslkey: s.useClientCertificate ? (s.clientKey ?? null) : null,
          keyPassword: s.useClientCertificate ? (s.keyPassphrase ?? null) : null,
          ...profileArgs(s),
        });
        if (prelogin.type !== "saml") {
          if (!connectCancelledRef.current) {
            setIsAuthenticating(false);
            unstick();
            if (showWindowOnFallback) await showWindow();
          }
          return;
        }
        await invoke("connect_saml", {
          portal: savedPortal,
          gateway: localStorage.getItem(GATEWAY_KEY) || null,
          browser: null,
          reuseAuthCookies: true,
          useExternalBrowser: s.useExternalBrowser ?? false,
          certificate: s.useClientCertificate ? (s.clientCertificate ?? null) : null,
          sslkey: s.useClientCertificate ? (s.clientKey ?? null) : null,
          keyPassword: s.useClientCertificate ? (s.keyPassphrase ?? null) : null,
          disableIpv6: s.disableIPv6 ?? false,
          noDtls: s.noDtls ?? false,
          hidpi: s.hidpi ?? false,
          ...profileArgs(s),
          ...tunnelArgs(s),
        });
        // connect_saml succeeded — connection proceeds in the background
      } catch {
        if (!connectCancelledRef.current) {
          setIsAuthenticating(false);
          unstick();
          if (showWindowOnFallback) await showWindow();
        }
      } finally {
        connectInFlightRef.current = false;
      }
    };

    // One-shot autoConnect-on-launch: consumed on the FIRST observed state
    // regardless of tag, so it can only ever fire at startup — never
    // mid-session (e.g. undoing an explicit user Disconnect when the GUI
    // launched while already connected).
    const maybeAutoConnect = (state: VpnState) => {
      if (autoConnectAttemptedRef.current) return;
      autoConnectAttemptedRef.current = true;
      if (getStateTag(state) !== "disconnected") return;
      const s = JSON.parse(localStorage.getItem("nusaprotect_settings") ?? "{}");
      if (s.autoConnect) void attemptAutoConnect(false);
    };

    const handleVpnState = (next: VpnState) => {
      vpnStateRef.current = next;
      if (typeof next === "object" && next !== null && "connected" in next) {
        const gws = next.connected.info.gateways;
        if (Array.isArray(gws) && gws.length > 0) {
          setGateways(gws);
          localStorage.setItem(GATEWAYS_CACHE_KEY, JSON.stringify(gws));
        }
      }
      if (typeof next === "object" && next !== null && "failed" in next) {
        setConnectError(next.failed);
      }
      setVpnState((prev) => {
        const wasConnected = typeof prev === "object" && prev !== null && "connected" in prev;
        const isConnected = typeof next === "object" && next !== null && "connected" in next;
        if (isConnected && !wasConnected) setConnectedAt(Date.now());
        return next;
      });
      setIsAuthenticating(false);

      // An episode boundary — a later reconnectRequired may reuse attempt
      // numbering, so it must not be deduped against the old key.
      const tag = getStateTag(next);
      if (tag === "connected" || tag === "disconnected") {
        reconnectAttemptedRef.current = null;
      }
      // Tunnel details only describe the live tunnel — drop stale rows when
      // leaving the connected view so a reconnect never shows the old session.
      if (tag !== "connected") setTunnelState(null);

      if (typeof next === "object" && next !== null && "reconnectRequired" in next) {
        // The service is explicitly asking the GUI to redo auth — not gated by
        // any setting. Attempt at most once per (owner, attempt) request so a
        // failing flow can't be re-driven indefinitely by the service.
        const key = `${next.reconnectRequired.owner}:${next.reconnectRequired.attempt}`;
        if (reconnectAttemptedRef.current !== key && localStorage.getItem(PORTAL_KEY)) {
          reconnectAttemptedRef.current = key;
          void attemptAutoConnect(true);
        } else {
          // Already tried this request once (or no portal saved): the service is
          // blocked awaiting re-auth — keep the connect form reachable instead
          // of stranding the UI on the connecting view.
          unstick();
        }
      }

      // Auto-connect on launch (once): window may be intentionally hidden via
      // startMinimized, so don't force it open on failure.
      maybeAutoConnect(next);
    };

    const unlistenState = listen<VpnState>("vpn-state", (event) => {
      handleVpnState(event.payload);
    });

    const unlistenServiceErr = listen<string>("service-error", (event) => {
      setServiceError(
        `Could not connect to gpservice: ${event.payload}. Is gpservice running?`
      );
    });

    const unlistenServiceConnected = listen<VpnState>("service-connected", (event) => {
      setServiceError(null);
      // The attach snapshot is bundled into this event because the "vpn-state"
      // emit during connect_to_service can race past our listener registration.
      // Replay it through the same handler so launch-state logic sees the real
      // state — never assume disconnected (the tunnel may already be up).
      if (vpnStateRef.current === null) handleVpnState(event.payload);
    });

    const unlistenRequestRejected = listen<string>("request-rejected", (event) => {
      setConnectError(`Request rejected by gpservice: ${event.payload}`);
    });

    const unlistenDisconnected = listen("service-disconnected", () => {
      setServiceError("Lost connection to gpservice.");
      setVpnState("disconnected");
      vpnStateRef.current = "disconnected";
      setIsAuthenticating(false);
      setTunnelState(null);
    });

    const unlistenChooseGateway = listen<Gateway[]>("choose-gateway", (event) => {
      setGatewayChoices(event.payload);
    });

    const unlistenTrayConnect = listen("tray-connect-requested", async () => {
      if (getStateTag(vpnStateRef.current) !== "disconnected") {
        // Service is already connecting/connected — a second auth flow would be wrong.
        await showWindow();
        return;
      }
      await attemptAutoConnect(true);
    });

    const unlistenTunnelState = listen<TunnelState | null>("tunnel-state", (event) => {
      setTunnelState(event.payload);
    });

    // gpclient launch-gui hits /active-gui on the service when a GUI is already
    // attached — the service asks us to raise our window.
    const unlistenActiveGui = listen("active-gui", () => {
      void showWindow();
    });

    const unlistenResume = listen("resume-connection", async () => {
      const s = JSON.parse(localStorage.getItem("nusaprotect_settings") ?? "{}");
      if (!(s.resumeOnWake ?? false)) return;
      // Same gate as tray-connect: only resume from a truly disconnected state —
      // a resume during connecting/reconnectRequired must not start a second flow.
      if (getStateTag(vpnStateRef.current) !== "disconnected") return;
      await attemptAutoConnect(true);
    });

    const unlistenCloseRequested = listen("window-close-requested", async () => {
      const s = JSON.parse(localStorage.getItem("nusaprotect_settings") ?? "{}");
      if (s.closeWindowBehavior === "quit") {
        await invoke("quit_app");
      } else {
        await invoke("minimize_window").catch(() => {});
      }
    });

    return () => {
      unlistenState.then((u) => u());
      unlistenChooseGateway.then((u) => u());
      unlistenServiceErr.then((u) => u());
      unlistenServiceConnected.then((u) => u());
      unlistenRequestRejected.then((u) => u());
      unlistenDisconnected.then((u) => u());
      unlistenTrayConnect.then((u) => u());
      unlistenTunnelState.then((u) => u());
      unlistenActiveGui.then((u) => u());
      unlistenResume.then((u) => u());
      unlistenCloseRequested.then((u) => u());
    };
  }, []);

  const handleGetPrelogin = useCallback(async (portal: string): Promise<void> => {
    setConnectError(null);
    try {
      const s = readSettings();
      const info = await invoke<PreloginType>("get_prelogin", {
        portal,
        certificate: s.useClientCertificate ? (s.clientCertificate ?? null) : null,
        sslkey: s.useClientCertificate ? (s.clientKey ?? null) : null,
        keyPassword: s.useClientCertificate ? (s.keyPassphrase ?? null) : null,
        ...profileArgs(s),
      });
      setPreloginInfo(info);
    } catch (e) {
      setConnectError(String(e));
    }
  }, []);

  const readSettings = () => JSON.parse(localStorage.getItem("nusaprotect_settings") ?? "{}");

  const handleConnectSaml = useCallback(async (portal: string, browser: string | null) => {
    if (connectInFlightRef.current) return;
    connectInFlightRef.current = true;
    connectCancelledRef.current = false;
    setIsAuthenticating(true);
    setConnectError(null);
    try {
      const s = readSettings();
      await invoke("connect_saml", {
        portal,
        gateway: gateway || null,
        browser,
        reuseAuthCookies: s.reuseAuthCookies ?? true,
        useExternalBrowser: s.useExternalBrowser ?? false,
        certificate: s.useClientCertificate ? (s.clientCertificate ?? null) : null,
        sslkey: s.useClientCertificate ? (s.clientKey ?? null) : null,
        keyPassword: s.useClientCertificate ? (s.keyPassphrase ?? null) : null,
        disableIpv6: s.disableIPv6 ?? false,
        noDtls: s.noDtls ?? false,
        hidpi: s.hidpi ?? false,
        ...profileArgs(s),
        ...tunnelArgs(s),
      });
    } catch (e) {
      if (!connectCancelledRef.current) {
        setConnectError(String(e));
        setIsAuthenticating(false);
      }
    } finally {
      connectInFlightRef.current = false;
    }
  }, [gateway]);

  const handleConnectPassword = useCallback(
    async (portal: string, username: string, password: string) => {
      if (connectInFlightRef.current) return;
      connectInFlightRef.current = true;
      connectCancelledRef.current = false;
      setIsAuthenticating(true);
      setConnectError(null);
      try {
        const s = readSettings();
        await invoke("connect_password", {
          portal,
          gateway: gateway || null,
          username,
          password,
          certificate: s.useClientCertificate ? (s.clientCertificate ?? null) : null,
          sslkey: s.useClientCertificate ? (s.clientKey ?? null) : null,
          keyPassword: s.useClientCertificate ? (s.keyPassphrase ?? null) : null,
          disableIpv6: s.disableIPv6 ?? false,
          noDtls: s.noDtls ?? false,
          ...profileArgs(s),
          ...tunnelArgs(s),
        });
      } catch (e) {
        if (!connectCancelledRef.current) {
          setConnectError(String(e));
          setIsAuthenticating(false);
        }
      } finally {
        connectInFlightRef.current = false;
      }
    },
    [gateway]
  );

  const handleDisconnect = useCallback(async () => {
    try {
      await invoke("disconnect");
    } catch (e) {
      console.error("disconnect:", e);
    }
  }, []);

  const handleSwitchGateway = useCallback(async (name: string) => {
    if (connectInFlightRef.current) return;
    try {
      setConnectError(null);
      await invoke("switch_gateway", { gateway: name });
      setGateway(name);
    } catch (e) {
      console.error("switch_gateway:", e);
      setConnectError(String(e));
    }
  }, []);

  const handleCancelConnect = useCallback(() => {
    connectCancelledRef.current = true;
    invoke("cancel_connect").catch(() => {});
    setIsAuthenticating(false);
    setPreloginInfo(null);
  }, []);

  const handleClearCredentials = useCallback(async () => {
    setMenuAnchor(null);
    try {
      await invoke("clear_credentials");
    } catch (e) {
      console.error("clear_credentials:", e);
    }
    setPortal("");
    setPreloginInfo(null);
    setConnectError(null);
  }, []);

  const handleOpenSettings = useCallback(async (section?: string) => {
    setMenuAnchor(null);
    try {
      await invoke("open_settings", { section: section ?? null });
    } catch (e) {
      console.error("open_settings:", e);
    }
  }, []);

  const stateTag = getStateTag(vpnState);
  const isConnecting = isAuthenticating || stateTag === "connecting";
  const isConnected = stateTag === "connected";

  const renderContent = () => {
    if (gatewayChoices) {
      return (
        <Box sx={{ flex: 1, display: "flex", flexDirection: "column", justifyContent: "center", gap: 1.5, px: 3 }}>
          <Typography variant="body1" color="text.secondary" sx={{ mb: 1 }}>
            Select a gateway
          </Typography>
          <Button
            variant="contained"
            fullWidth
            onClick={() => {
              setGatewayChoices(null);
              invoke("select_gateway", { gateway: "" }).catch(() => {});
            }}
          >
            Auto (recommended)
          </Button>
          {gatewayChoices.map((g) => (
            <Button
              key={g.name}
              variant="outlined"
              fullWidth
              onClick={() => {
                setGatewayChoices(null);
                invoke("select_gateway", { gateway: g.name }).catch(() => {});
              }}
            >
              {g.name}
            </Button>
          ))}
          <Button
            size="small"
            onClick={() => {
              setGatewayChoices(null);
              invoke("select_gateway", { gateway: null }).catch(() => {});
            }}
          >
            Cancel
          </Button>
        </Box>
      );
    }
    if (stateTag === "disconnecting") return <DisconnectingView />;
    if (isConnecting) {
      return (
        <ConnectingView isAuthenticating={isAuthenticating} onCancel={handleCancelConnect} />
      );
    }
    if (isConnected && vpnState && typeof vpnState === "object" && "connected" in vpnState) {
      return (
        <ConnectedView
          portal={vpnState.connected.info.portal}
          gatewayName={vpnState.connected.info.gateway.name}
          gatewayAddress={vpnState.connected.info.gateway.address}
          gateways={gateways}
          connectedAt={connectedAt}
          tunnel={tunnelState}
          onDisconnect={handleDisconnect}
          onSwitchGateway={handleSwitchGateway}
        />
      );
    }
    return (
      <DisconnectedView
        portal={portal}
        onPortalChange={setPortal}
        gateway={gateway}
        gateways={gateways}
        onGatewayChange={setGateway}
        error={connectError}
        onOpenSettings={() => handleOpenSettings("ssl")}
        preloginInfo={preloginInfo}
        onGetPrelogin={handleGetPrelogin}
        onConnectSaml={handleConnectSaml}
        onConnectPassword={handleConnectPassword}
      />
    );
  };

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <Box sx={{ height: "100vh", display: "flex", flexDirection: "column" }}>
        {/* Header */}
        <Box
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) invoke("start_dragging").catch(() => {});
          }}
          sx={{
            display: "flex",
            alignItems: "center",
            pl: 0.5,
            height: 36,
            borderBottom: 1,
            borderColor: "divider",
            userSelect: "none",
          }}
        >
          <IconButton size="small" onClick={(e) => setMenuAnchor(e.currentTarget)}>
            <MenuIcon fontSize="small" />
          </IconButton>
          <Typography
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) invoke("start_dragging").catch(() => {});
            }}
            variant="caption"
            color="text.secondary"
            sx={{ flex: 1, textAlign: "center", fontWeight: 600, letterSpacing: 0.5 }}
          >
            NusaProtect
          </Typography>
          <IconButton
            size="small"
            onClick={() => invoke("open_url", { url: "https://github.com/jahrulnr/NusaProtect" })}
          >
            <GitHubIcon fontSize="small" />
          </IconButton>
          <IconButton size="small" onClick={() => invoke("minimize_window").catch(() => {})}>
            <RemoveIcon fontSize="small" />
          </IconButton>
          <IconButton size="small" onClick={() => invoke("close_window").catch(() => {})} sx={{ mr: 0.5 }}>
            <CloseIcon fontSize="small" />
          </IconButton>
        </Box>

        {/* Hamburger menu */}
        <Menu
          anchorEl={menuAnchor}
          open={Boolean(menuAnchor)}
          onClose={() => setMenuAnchor(null)}
        >
          <MenuItem onClick={handleClearCredentials}>
            <ListItemIcon><DeleteOutlineIcon fontSize="small" /></ListItemIcon>
            <ListItemText>Clear Credentials</ListItemText>
          </MenuItem>
          <MenuItem onClick={() => handleOpenSettings()}>
            <ListItemIcon><SettingsIcon fontSize="small" /></ListItemIcon>
            <ListItemText>Settings</ListItemText>
          </MenuItem>
          <MenuItem onClick={() => { setMenuAnchor(null); invoke("quit_app"); }}>
            <ListItemIcon><PowerSettingsNewIcon fontSize="small" /></ListItemIcon>
            <ListItemText>Quit</ListItemText>
          </MenuItem>
        </Menu>

        {/* Main content */}
        <Box sx={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
          {renderContent()}
        </Box>

        {/* Footer */}
        <Box sx={{ textAlign: "center", py: 0.75 }}>
          <Typography variant="caption" color="text.disabled">
            {version ? `v${version}` : ""}
          </Typography>
        </Box>
      </Box>

      <Snackbar
        open={!!serviceError}
        autoHideDuration={8000}
        onClose={() => setServiceError(null)}
        anchorOrigin={{ vertical: "bottom", horizontal: "center" }}
      >
        <Alert severity="error" onClose={() => setServiceError(null)}>
          {serviceError}
        </Alert>
      </Snackbar>
    </ThemeProvider>
  );
}
