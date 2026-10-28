export interface Gateway {
  name: string;
  address: string;
  priority: number;
  priorityRules: { name: string; priority: number }[];
}

export interface ConnectInfo {
  portal: string;
  gateway: Gateway;
  gateways: Gateway[];
}

export interface SessionInfo {
  lifetimeSecs?: number;
  userExpires?: number;
  expiresInHuman?: string;
  allowExtendSession?: boolean;
}

// Matches Rust ConnectedInfo (VpnState::Connected payload in gpservice 3.2.x)
export interface ConnectedInfo {
  info: ConnectInfo;
  sessionInfo: SessionInfo | null;
  connectedAt: number;
  members: unknown[];
  failures: unknown[];
}

// Matches Rust ReconnectRequest (VpnState::ReconnectRequired payload in gpservice 3.2.x)
export interface ReconnectRequest {
  attempt: number;
  owner: string;
  info: ConnectInfo;
}

// Matches Rust VpnState enum with serde(rename_all = "camelCase")
export type VpnState =
  | "disconnected"
  | "disconnecting"
  | { connecting: ConnectInfo }
  | { connected: ConnectedInfo }
  | { reconnectRequired: ReconnectRequest }
  | { failed: string };

export type PreloginType =
  | { type: "saml" }
  | {
      type: "standard";
      authMessage: string;
      labelUsername: string;
      labelPassword: string;
    };

// Matches Rust TunnelPhase / TunnelTransport (serde camelCase)
export type TunnelPhase = "establishing" | "connected" | "reconnecting" | "ended";
export type TunnelTransport = "negotiating" | "ipsecEsp" | "sslTls" | "unavailable";

export interface TunnelList {
  items: string[];
  totalCount: number;
  truncated: boolean;
}

export interface TunnelAddress {
  address: string;
  netmask?: string;
  prefixLength?: number;
}

export interface TunnelConfiguration {
  interface?: string;
  ipv4?: TunnelAddress;
  ipv6?: TunnelAddress;
  serverIp?: string;
  dns: TunnelList;
  searchDomains: TunnelList;
  splitDns: TunnelList;
  includedRoutes: TunnelList;
  excludedRoutes: TunnelList;
  mtu?: number;
  truncated: boolean;
}

// Matches Rust TunnelState; u64 fields serialize as decimal STRINGS.
export interface TunnelState {
  tunnelId: string;
  sequence: string;
  phase: TunnelPhase;
  configurationRevision: string;
  configuration?: TunnelConfiguration;
  transport: TunnelTransport;
  elapsedMs: string;
}
