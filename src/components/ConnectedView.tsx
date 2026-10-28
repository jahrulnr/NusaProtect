import { useState, useEffect, ReactNode } from "react";
import { Box, Button, Collapse, Divider, MenuItem, Stack, TextField, Tooltip, Typography } from "@mui/material";
import VpnLock from "@mui/icons-material/VpnLock";
import BoltIcon from "@mui/icons-material/Bolt";
import ShieldOutlinedIcon from "@mui/icons-material/ShieldOutlined";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import { Gateway, TunnelConfiguration, TunnelState, TunnelTransport } from "../types";

function useElapsed(connectedAt: number): string {
  const [elapsed, setElapsed] = useState(Date.now() - connectedAt);

  useEffect(() => {
    const id = setInterval(() => setElapsed(Date.now() - connectedAt), 1000);
    return () => clearInterval(id);
  }, [connectedAt]);

  const totalSeconds = Math.floor(elapsed / 1000);
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = totalSeconds % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

// Friendly label for the negotiated transport. Layperson framing: how the VPN
// is carrying traffic, not protocol jargon.
const TRANSPORT_INFO: Record<TunnelTransport, { label: string; hint: string; express?: boolean } | null> = {
  ipsecEsp: { label: "Express", hint: "Using the fastest VPN path", express: true },
  sslTls: { label: "Standard", hint: "Reliable path — may be a little slower" },
  negotiating: { label: "Optimizing…", hint: "Picking the best path" },
  unavailable: null,
};

const DEFAULT_ROUTES = ["0.0.0.0/0", "::/0"];
function isSplitTunnel(config: TunnelConfiguration | undefined): boolean {
  if (!config) return false;
  if (config.excludedRoutes.totalCount > 0) return true;
  const included = config.includedRoutes;
  return included.totalCount > 0 && !included.items.some((r) => DEFAULT_ROUTES.includes(r));
}

interface Props {
  portal: string;
  gatewayName: string;
  gatewayAddress: string;
  gateways: Gateway[];
  connectedAt: number;
  tunnel?: TunnelState | null;
  onDisconnect: () => void;
  onSwitchGateway: (name: string) => void;
}

function Row({ label, children, caption }: { label: string; children: ReactNode; caption?: boolean }) {
  const variant = caption ? "caption" : "body2";
  return (
    <Box sx={{ display: "flex", justifyContent: "space-between", gap: 2, alignItems: "baseline" }}>
      <Typography variant={variant} color="text.secondary" sx={{ flexShrink: 0 }}>{label}</Typography>
      <Box sx={{ wordBreak: "break-all", textAlign: "right", minWidth: 0 }}>{children}</Box>
    </Box>
  );
}

export default function ConnectedView({ portal, gatewayName, gatewayAddress, gateways, connectedAt, tunnel, onDisconnect, onSwitchGateway }: Props) {
  const elapsed = useElapsed(connectedAt);
  const [showAdvanced, setShowAdvanced] = useState(false);

  // "ended" phase = no live tunnel, even if the event still carries config.
  const live = tunnel && tunnel.phase !== "ended" ? tunnel : null;
  const config = live?.configuration;
  const transport = live ? TRANSPORT_INFO[live.transport] : null;
  const split = isSplitTunnel(config);

  const dnsSummary = config && config.dns.totalCount > 0
    ? config.dns.items.slice(0, 2).join(", ") + (config.dns.totalCount > 2 ? ` +${config.dns.totalCount - 2}` : "")
    : null;

  return (
    <Box
      sx={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "safe center",
        gap: 1.5,
        px: 3,
        py: 1,
        overflowY: "auto",
      }}
    >
      <Box
        sx={{
          width: 80,
          height: 80,
          borderRadius: "50%",
          bgcolor: "rgba(76,175,80,0.12)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <VpnLock sx={{ fontSize: 40, color: "success.main" }} />
      </Box>

      <Box sx={{ textAlign: "center" }}>
        <Typography variant="body2" color="success.light" noWrap sx={{ maxWidth: 280 }}>
          {gatewayName}
        </Typography>
        <Typography variant="h6" color="success.main">
          Connected
        </Typography>
        <Box
          sx={{
            display: "inline-block",
            mt: 0.5,
            px: 1.5,
            py: 0.25,
            borderRadius: 99,
            bgcolor: "rgba(76,175,80,0.12)",
          }}
        >
          <Typography variant="caption" color="success.main" sx={{ fontVariantNumeric: "tabular-nums" }}>
            {elapsed}
          </Typography>
        </Box>
      </Box>

      <Stack
        spacing={1}
        sx={{ width: "100%", bgcolor: "action.hover", borderRadius: 2, p: 2, minWidth: 0 }}
      >
        <Row label="Portal">{portal}</Row>
        <Divider />
        <Row label="Gateway">{gatewayName}</Row>
        <Row label="Address">{gatewayAddress}</Row>

        {live && (
          <>
            <Divider />
            <Button
              size="small"
              onClick={() => setShowAdvanced((v) => !v)}
              aria-expanded={showAdvanced}
              endIcon={
                <ExpandMoreIcon
                  sx={{
                    fontSize: 16,
                    transition: "transform 0.2s",
                    transform: showAdvanced ? "rotate(180deg)" : "none",
                  }}
                />
              }
              sx={{ alignSelf: "flex-end", minWidth: 0, p: 0, textTransform: "none", color: "text.secondary" }}
            >
              Details
            </Button>

            <Collapse in={showAdvanced} unmountOnExit>
              <Stack spacing={1}>
                {transport && (
                  <Row label="Connection">
                    <Tooltip title={transport.hint} placement="left" arrow>
                      <Stack direction="row" spacing={0.5} alignItems="center" component="span">
                        {transport.express
                          ? <BoltIcon sx={{ fontSize: 15, color: "warning.main" }} />
                          : <ShieldOutlinedIcon sx={{ fontSize: 15, color: "text.secondary" }} />}
                        <Typography variant="body2" component="span">{transport.label}</Typography>
                      </Stack>
                    </Tooltip>
                  </Row>
                )}

                {config?.ipv4 && (
                  <Row label="VPN address">{config.ipv4.address}</Row>
                )}

                {config && (
                  <Row label="Protection">
                    {split ? (
                      <Tooltip title="Only some traffic goes through the VPN" placement="left" arrow>
                        <Typography variant="body2" component="span">Split tunneling</Typography>
                      </Tooltip>
                    ) : (
                      "All traffic"
                    )}
                  </Row>
                )}

                {live.phase === "reconnecting" && (
                  <Typography variant="caption" color="warning.main" sx={{ textAlign: "right" }}>
                    Re-establishing…
                  </Typography>
                )}
                {live.phase === "establishing" && (
                  <Typography variant="caption" color="warning.main" sx={{ textAlign: "right" }}>
                    Establishing…
                  </Typography>
                )}

                {config && (
                  <>
                    <Divider />
                    {config.interface && <Row caption label="Interface"><Typography variant="caption">{config.interface}</Typography></Row>}
                    {config.serverIp && <Row caption label="Server"><Typography variant="caption">{config.serverIp}</Typography></Row>}
                    {config.ipv6 && <Row caption label="IPv6"><Typography variant="caption">{config.ipv6.address}</Typography></Row>}
                    {config.mtu != null && <Row caption label="MTU"><Typography variant="caption">{config.mtu}</Typography></Row>}
                    {dnsSummary && <Row caption label="DNS"><Typography variant="caption">{dnsSummary}</Typography></Row>}
                    {config.searchDomains.totalCount > 0 && (
                      <Row caption label="Domains"><Typography variant="caption">{config.searchDomains.items.join(", ")}</Typography></Row>
                    )}
                    {config.splitDns.totalCount > 0 && (
                      <Row caption label="Split DNS"><Typography variant="caption">{config.splitDns.items.join(", ")}</Typography></Row>
                    )}
                    <Row caption label="Routes">
                      <Typography variant="caption">
                        {config.includedRoutes.totalCount} included · {config.excludedRoutes.totalCount} excluded
                        {config.truncated ? " (partial)" : ""}
                      </Typography>
                    </Row>
                  </>
                )}
              </Stack>
            </Collapse>
          </>
        )}
      </Stack>

      {gateways.length > 1 && (
        <TextField
          select
          label="Switch gateway"
          value={gatewayName}
          onChange={(e) => {
            if (e.target.value !== gatewayName) onSwitchGateway(e.target.value);
          }}
          size="small"
          fullWidth
        >
          {gateways.map((g) => (
            <MenuItem key={g.name} value={g.name}>
              {g.name}
            </MenuItem>
          ))}
        </TextField>
      )}

      <Button variant="outlined" color="error" size="large" onClick={onDisconnect} fullWidth>
        Disconnect
      </Button>
    </Box>
  );
}
