// mobile.v2 wire types. Source of truth: flockr docs/mobile/protocol-v1.md
// (messages) plus the v2 Noise transport (see noise.ts).
// No React or React Native imports in src/protocol/: it also runs under Node.

export const PROTOCOL_VERSION = 2;
export const SUBPROTOCOL = 'flockr.mobile.v2';
export const WS_PATH = '/mobile/v2';
/** Largest server → client JSON message (v1 section 7). */
export const MAX_SERVER_MESSAGE_BYTES = 4 * 1024 * 1024;

export type AgentStatus = 'idle' | 'working' | 'blocked' | 'done' | 'unknown';

export function normalizeStatus(value: unknown): AgentStatus {
  switch (value) {
    case 'idle':
    case 'working':
    case 'blocked':
    case 'done':
      return value;
    default:
      return 'unknown';
  }
}

// --- Pairing payload (section 6) ---

export interface PairingPayload {
  v: number;
  name: string;
  hosts: string[];
  port: number;
  sid: string;
  /** Server Noise static public key, base64url without padding (43 chars). */
  pk: string;
  code: string;
  exp: number;
}

// --- Handshake (section 8) ---

export interface AppInfo {
  version: string;
  platform: string;
  build?: string;
}

export interface HelloAuth {
  device_id: string;
  token: string;
}

export interface HelloPair {
  code: string;
  device_name: string;
  sid: string;
}

export interface HelloMessage {
  t: 'hello';
  versions: number[];
  app: AppInfo;
  auth?: HelloAuth;
  pair?: HelloPair;
  max_in_flight?: number;
}

export interface WelcomeServer {
  name: string;
  server_id: string;
  flockr_version?: string;
  boot_id?: string;
}

export interface WelcomeDevice {
  device_id: string;
  name: string;
  token?: string;
}

export interface WelcomeLimits {
  max_in_flight?: number;
  max_subscriptions?: number;
  max_message_bytes?: number;
  max_input_bytes?: number;
  max_cols?: number;
  max_rows?: number;
}

export interface WelcomeMessage {
  t: 'welcome';
  version: number;
  server: WelcomeServer;
  device: WelcomeDevice;
  features: string[];
  methods?: string[];
  limits?: WelcomeLimits;
}

export type ErrorCode =
  | 'bad_request'
  | 'unknown_type'
  | 'unauthorized'
  | 'pairing_failed'
  | 'too_many_devices'
  | 'unsupported_version'
  | 'revoked'
  | 'too_many_connections'
  | 'rate_limited'
  | 'method_not_allowed'
  | 'not_found'
  | 'pane_busy'
  | 'too_many_subscriptions'
  | 'payload_too_large'
  | 'push_unavailable'
  | 'server_unavailable'
  | 'internal'
  | string;

export interface ErrorMessage {
  t: 'error';
  id?: string;
  code: ErrorCode;
  message?: string;
  supported?: number[];
}

// --- Close codes (section 7) ---

export const CloseCode = {
  normal: 1000,
  tooBig: 1009,
  protocolError: 4000,
  unauthorized: 4001,
  unsupportedVersion: 4002,
  revoked: 4003,
  tooManyConnections: 4004,
  pairingFailed: 4005,
  /** v2: the Noise handshake failed (wrong key or a different machine). */
  handshakeFailed: 4006,
  helloTimeout: 4008,
  serverShutdown: 4009,
  rateLimited: 4029,
} as const;

/** Close codes after which the app must not reconnect on its own. */
export const TERMINAL_CLOSE_CODES: ReadonlySet<number> = new Set([
  CloseCode.unauthorized,
  CloseCode.revoked,
  CloseCode.pairingFailed,
  CloseCode.unsupportedVersion,
  CloseCode.handshakeFailed,
]);

/** Close codes after which the stored device token is dead. */
export const TOKEN_DEAD_CLOSE_CODES: ReadonlySet<number> = new Set([CloseCode.unauthorized, CloseCode.revoked]);

// --- API passthrough (section 9.2) ---

export interface ApiError {
  code: string;
  message: string;
}

export interface ApiResponseLine {
  id?: string;
  result?: Record<string, unknown> & { type?: string };
  error?: ApiError;
}

export interface ApiEventLine {
  event: string;
  data?: Record<string, unknown>;
}

// --- Agents (section 9.3) ---

export interface AgentInfo {
  terminal_id: string;
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  name?: string;
  agent?: string;
  title?: string;
  terminal_title?: string;
  terminal_title_stripped?: string;
  display_agent?: string;
  agent_status: AgentStatus;
  state_change_seq?: number;
  revision?: number;
  focused?: boolean;
  cwd?: string;
  foreground_cwd?: string;
  /** Git branch of the agent's checkout; full name (strip `worktree/` for display). */
  branch?: string;
  /** The agent is on a fresh session it has not worked in yet. */
  session_empty?: boolean;
  [key: string]: unknown;
}

export interface WorkspaceInfo {
  workspace_id: string;
  number: number;
  label: string;
  focused?: boolean;
  agent_status: AgentStatus;
  active_tab_id?: string;
  pane_count?: number;
  tab_count?: number;
  [key: string]: unknown;
}

export interface TabInfo {
  tab_id: string;
  workspace_id: string;
  number: number;
  label: string;
  focused?: boolean;
  pane_count?: number;
  agent_status: AgentStatus;
  [key: string]: unknown;
}

export interface AgentsMessage {
  t: 'agents';
  id: string;
  rev: number;
  agents: AgentInfo[];
  workspaces: WorkspaceInfo[];
  tabs: TabInfo[];
}

// --- Surface (sections 9.4, 9.5, 10) ---

export type SurfaceMode = 'observe' | 'fit';

/** [fg, bg, flags] or [fg, bg, flags, linkIndex]. */
export type Style = [number, number, number] | [number, number, number, number];

/** [x, styleId, text] (1 col per UTF-16 unit) or [x, styleId, grapheme, width]. */
export type Run = [number, number, string] | [number, number, string, number];

export type Line = Run[];

export interface Cursor {
  x: number;
  y: number;
  visible: boolean;
  shape: number;
}

export interface ScrollInfo {
  offset: number;
  max: number;
  viewport: number;
}

export interface Theme {
  fg?: string | null;
  bg?: string | null;
  palette?: (string | null)[];
}

export interface SurfaceStarted {
  t: 'surface.started';
  id?: string;
  sub: number;
  pane_id: string;
  terminal_id: string;
  mode: SurfaceMode | string;
  cols: number;
  rows: number;
  theme?: Theme;
}

export interface SurfaceFrame {
  t: 'surface.frame';
  sub: number;
  seq: number;
  cols: number;
  rows: number;
  styles: Style[];
  links?: string[];
  lines: Line[];
  cursor?: Cursor | null;
  scroll?: ScrollInfo | null;
  alt?: boolean;
}

export interface SurfaceDelta {
  t: 'surface.delta';
  sub: number;
  seq: number;
  base: number;
  shift?: number;
  styles_add?: Style[];
  links_add?: string[];
  lines?: [number, Line][];
  cursor?: Cursor | null;
  scroll?: ScrollInfo | null;
  alt?: boolean;
}

export type SurfaceEndReason =
  'unsubscribed' | 'pane_closed' | 'revoked' | 'frame_too_large' | 'taken_over' | 'server_shutdown' | 'error';

export function normalizeEndReason(value: unknown): SurfaceEndReason {
  switch (value) {
    case 'unsubscribed':
    case 'pane_closed':
    case 'revoked':
    case 'frame_too_large':
    case 'taken_over':
    case 'server_shutdown':
      return value;
    default:
      return 'error';
  }
}

export interface SurfaceEnded {
  t: 'surface.ended';
  sub: number;
  reason: string;
}

export interface SurfaceHistoryResult {
  t: 'surface.history.result';
  id: string;
  sub: number;
  lines: string[];
}

// --- Notify (section 9.8) ---

export interface NotifyMessage {
  t: 'notify';
  kind: 'blocked' | 'done' | string;
  pane_id: string;
  workspace_id?: string;
  tab_id?: string;
  terminal_id?: string;
  title: string;
  body: string;
}

/** Push `data` payload (section 11.4). */
export interface PushData {
  type: 'agent_status' | string;
  server_id: string;
  server_name?: string;
  status: string;
  agent?: string;
  workspace_id?: string;
  workspace?: string;
  tab_id?: string;
  tab?: string;
  pane_id: string;
  terminal_id?: string;
}

/** Any message from the server. `t` is checked at runtime. */
export interface ServerMessage {
  t: string;
  id?: string;
  [key: string]: unknown;
}

export const EXPO_PUSH_TOKEN_RE = /^(Exponent|Expo)PushToken\[[A-Za-z0-9_-]{1,256}\]$/;
