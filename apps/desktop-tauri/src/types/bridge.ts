export type SurfaceMode = "hidden" | "trayPanel" | "popOut" | "settings";
export type VisibleSurfaceMode = Exclude<SurfaceMode, "hidden">;
export type SettingsTabId =
  | "general"
  | "providers"
  | "notifications"
  | "menuBar"
  | "menu"
  | "usageSpend"
  | "advanced"
  | "about";

// ── Narrowed string-literal unions (persisted settings enums) ─────────

export type TrayIconMode = "single" | "perProvider";

export type NotificationSoundTheme = "windows" | "codexBar";

export type NotificationSoundEvent =
  | "predictiveWarning"
  | "highUsage"
  | "criticalUsage"
  | "exhausted"
  | "statusIssue"
  | "sessionDepleted"
  | "sessionRestored";

export interface NotificationSoundPaths {
  predictiveWarning: string | null;
  highUsage: string | null;
  criticalUsage: string | null;
  exhausted: string | null;
  statusIssue: string | null;
  sessionDepleted: string | null;
  sessionRestored: string | null;
}

export type MetricPreference =
  | "automatic"
  | "session"
  | "weekly"
  | "model"
  | "tertiary"
  | "credits"
  | "extraUsage"
  | "average";

export type Language =
  | "english"
  | "chinese"
  | "chinesetraditional"
  | "japanese"
  | "korean"
  | "spanish"
  | "russian";

/** Language catalog entry from the Rust backend. */
export type LanguageOption = {
  /** Stable bridge/settings value (e.g. "english") */
  value: Language;
  /** Native display name (e.g. "English", "中文", "Español") */
  display: string;
};

export type UpdateChannel = "stable" | "beta";

export type ThemePreference = "auto" | "light" | "dark";

export type MenuBarDisplayMode = "minimal" | "compact" | "detailed";
export type FloatBarOrientation = "horizontal" | "vertical";
export type FloatBarStyle = "floating" | "taskbar";

export type TrayVisibilitySupport = "supported" | "unsupportedOs";
export type TrayVisibilityState = "promoted" | "notPromoted" | "entryNotFound" | "unknown";

export type DeepSeekPricingPeriod = "standard" | "peak" | "offPeak";

export interface DeepSeekPricingStatus {
  period: DeepSeekPricingPeriod;
  currentLocalTime: string;
  nextTransitionLocalTime: string | null;
  effectiveLocalTime: string;
}

export interface TrayVisibilityStatusDto {
  support: TrayVisibilitySupport;
  state: TrayVisibilityState;
}

export type TrayPanelSurfaceTarget = { kind: "summary" };
export type PopOutSurfaceTarget =
  | { kind: "dashboard" }
  | { kind: "provider"; providerId: string };
export type SettingsSurfaceTarget = { kind: "settings"; tab: SettingsTabId };

export type SurfaceTarget =
  | TrayPanelSurfaceTarget
  | PopOutSurfaceTarget
  | SettingsSurfaceTarget;

export type SurfaceTargetForMode<M extends VisibleSurfaceMode> =
  M extends "trayPanel"
    ? TrayPanelSurfaceTarget
    : M extends "popOut"
      ? PopOutSurfaceTarget
      : SettingsSurfaceTarget;

export interface CurrentSurfaceState {
  mode: SurfaceMode;
  target: SurfaceTarget;
}

export interface AgentSession {
  id: string;
  provider: "codex" | "claude" | "pi";
  /** Pi-family dialect (upstream 0.48.0 #2626); absent for Codex/Claude. */
  dialect?: "pi" | "omp";
  /** Optional session title (Pi-family `session_info`/`title` records). */
  sessionName?: string;
  source: "cli" | "desktopApp" | "ide" | "unknown";
  state: "active" | "idle";
  pid: number | null;
  transcriptPath: string | null;
  host: string;
  workspace: {
    cwd: string | null;
    projectName: string | null;
  };
  activity: {
    startedAt: string | null;
    lastActivityAt: string | null;
  };
  focusTarget:
    | { kind: "process"; pid: number }
    | { kind: "transcript"; transcriptPath: string }
    | { kind: "none" };
}

export interface AgentSessionHostResult {
  host: string;
  sessions: AgentSession[];
  error: string | null;
}

export type AgentSessionDiscoveryResult =
  | { status: "disabled" }
  | { status: "hosts"; hosts: AgentSessionHostResult[] };

export type SessionFocusResult =
  | { status: "focused" }
  | { status: "unsupported"; message: string }
  | { status: "failed"; message: string };

export interface ProviderCatalogEntry {
  id: string;
  displayName: string;
  cookieDomain: string | null;
}

export interface ProviderSummary {
  id: string;
  displayName: string;
  enabled: boolean;
  order: number;
}

export interface SettingsSnapshot {
  enabledProviders: string[];
  providerOrder?: string[];
  refreshIntervalSecs: number;
  adaptiveRefresh: boolean;
  refreshAllProvidersOnMenuOpen: boolean;
  lowPowerMode: boolean;
  startAtLogin: boolean;
  startMinimized: boolean;
  showNotifications: boolean;
  soundEnabled: boolean;
  notificationSoundTheme: NotificationSoundTheme;
  notificationSoundPaths: NotificationSoundPaths;
  highUsageThreshold: number;
  criticalUsageThreshold: number;
  providerUsageThresholds?: Record<string, UsageThresholdOverride>;
  predictivePaceWarningEnabled: boolean;
  trayIconMode: TrayIconMode;
  switcherShowsIcons: boolean;
  menuBarShowsHighestUsage: boolean;
  menuBarShowsPercent: boolean;
  showAsUsed: boolean;
  showAllTokenAccountsInMenu: boolean;
  enableAnimations: boolean;
  resetTimeRelative: boolean;
  showResetWhenExhausted: boolean;
  menuBarDisplayMode: MenuBarDisplayMode;
  hidePersonalInfo: boolean;
  updateChannel: UpdateChannel;
  autoDownloadUpdates: boolean;
  installUpdatesOnQuit: boolean;
  globalShortcut: string;
  /** Extra Codex home or sessions directories scanned for local cost estimates. */
  codexCustomSessionsDirs: string[];
  agentSessionsEnabled?: boolean;
  agentSessionSshHosts?: string[];
  /** Master switch for external hooks (hooks.json next to settings). */
  hooksEnabled?: boolean;
  /** Route provider HTTPS through a user HTTP(S) proxy (#235). */
  httpProxyEnabled?: boolean;
  httpProxyUrl?: string;
  httpProxyUsername?: string;
  httpProxyPassword?: string;
  uiLanguage: Language;
  theme: ThemePreference;
  /** 100..=250 — clamped server-side. */
  windowScalePercent: number;
  /** 100..=200 — clamped server-side. */
  trayScalePercent: number;
  powertoysStatusPipeEnabled: boolean;
  claudeAvoidKeychainPrompts: boolean;
  codexSparkUsageVisible: boolean;
  disableKeychainAccess: boolean;
  wayfinderGatewayUrl?: string;
  providerMetrics: Record<string, MetricPreference>;
  floatBarEnabled: boolean;
  /** 30..=100 — clamped server-side. */
  floatBarOpacity: number;
  /** 75..=200 — clamped server-side. */
  floatBarScale: number;
  floatBarOrientation: FloatBarOrientation;
  floatBarStyle: FloatBarStyle;
  floatBarClickThrough: boolean;
  /** Empty array = show all enabled providers. */
  floatBarProviderIds: string[];
  /** When true, render with dark text/glass for light desktops. */
  floatBarDarkText: boolean;
  /** When true, render the next primary reset inline in each provider pill. */
  floatBarShowResetInline: boolean;
  /** When true, scan and render local cost summaries. */
  floatBarShowCost: boolean;
  /** Promote the tray icon out of the Windows hidden-icons overflow (Win11 only). */
  promoteTrayIcon?: boolean;
  /** When true, show Claude Daily Routines quota row (default true). */
  claudeDailyRoutinesUsageVisible: boolean;
  /** Alibaba Token Plan region: cn | intl | cn-personal | intl-personal. */
  alibabaTokenPlanRegion: string;
  /** Optional work-week length [2,6] for session-equivalent weekly forecast. */
  weeklyProgressWorkDays?: number | null;
}

/** Partial settings object — only include fields you want to change. */
export interface SettingsUpdate {
  enabledProviders?: string[];
  refreshIntervalSecs?: number;
  adaptiveRefresh?: boolean;
  refreshAllProvidersOnMenuOpen?: boolean;
  lowPowerMode?: boolean;
  startAtLogin?: boolean;
  startMinimized?: boolean;
  showNotifications?: boolean;
  soundEnabled?: boolean;
  notificationSoundTheme?: NotificationSoundTheme;
  notificationSoundPaths?: NotificationSoundPaths;
  highUsageThreshold?: number;
  criticalUsageThreshold?: number;
  providerUsageThresholds?: Record<string, UsageThresholdOverride>;
  predictivePaceWarningEnabled?: boolean;
  trayIconMode?: TrayIconMode;
  switcherShowsIcons?: boolean;
  menuBarShowsHighestUsage?: boolean;
  menuBarShowsPercent?: boolean;
  showAsUsed?: boolean;
  showAllTokenAccountsInMenu?: boolean;
  enableAnimations?: boolean;
  resetTimeRelative?: boolean;
  showResetWhenExhausted?: boolean;
  menuBarDisplayMode?: MenuBarDisplayMode;
  hidePersonalInfo?: boolean;
  updateChannel?: UpdateChannel;
  autoDownloadUpdates?: boolean;
  installUpdatesOnQuit?: boolean;
  globalShortcut?: string;
  codexCustomSessionsDirs?: string[];
  agentSessionsEnabled?: boolean;
  agentSessionSshHosts?: string[];
  hooksEnabled?: boolean;
  httpProxyEnabled?: boolean;
  httpProxyUrl?: string;
  httpProxyUsername?: string;
  httpProxyPassword?: string;
  uiLanguage?: Language;
  theme?: ThemePreference;
  windowScalePercent?: number;
  trayScalePercent?: number;
  powertoysStatusPipeEnabled?: boolean;
  claudeAvoidKeychainPrompts?: boolean;
  codexSparkUsageVisible?: boolean;
  disableKeychainAccess?: boolean;
  /** Map of provider CLI name → metric preference label. */
  providerMetrics?: Record<string, MetricPreference>;
  floatBarEnabled?: boolean;
  floatBarOpacity?: number;
  floatBarScale?: number;
  floatBarOrientation?: FloatBarOrientation;
  floatBarStyle?: FloatBarStyle;
  floatBarClickThrough?: boolean;
  floatBarProviderIds?: string[];
  floatBarDarkText?: boolean;
  floatBarShowResetInline?: boolean;
  floatBarShowCost?: boolean;
  promoteTrayIcon?: boolean;
  claudeDailyRoutinesUsageVisible?: boolean;
  alibabaTokenPlanRegion?: string;
  weeklyProgressWorkDays?: number | null;
}

export interface UsageThresholdOverride {
  high?: number;
  critical?: number;
}

/** One provider row for Settings → Usage & Spend. */
export interface UsageSpendRow {
  providerId: string;
  displayName: string;
  sevenDay: number | null;
  thirtyDay: number | null;
  currency: string;
  source: string;
  /** F8: true when served from stale cache while a re-scan is in progress. */
  refreshing?: boolean;
  /** ISO 8601 timestamp of the stale snapshot when refreshing. */
  staleUpdatedAt?: string;
}

export interface UsageSpendSummary {
  rows: UsageSpendRow[];
}

/** Codex local Workspaces snapshot (get_codex_workspaces_snapshot). */
export type CodexWorkspacesSourceStatus =
  | "complete"
  | "catalogMissing"
  | "catalogLocked"
  | "catalogCorrupt"
  | "catalogIncompatible";

export interface CodexWorkspacesUsageTotals {
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  totalTokens: number;
}

export interface CodexWorkspacesCostEstimate {
  knownUsd: number;
  unknownTokens: number;
}

export interface CodexWorkspacesDailyPoint {
  day: string;
  totalTokens: number;
  cachedInputTokens: number;
  estimatedCostUsd: number | null;
}

export interface CodexWorkspacesSessionUsage {
  id: string;
  projectId: string;
  displayTitle: string;
  cwd: string | null;
  startedAt: string | null;
  latestActivity: string | null;
  totals: CodexWorkspacesUsageTotals;
  costEstimate: CodexWorkspacesCostEstimate;
  topModel: string | null;
}

export interface CodexWorkspacesProjectUsage {
  id: string;
  displayName: string;
  path: string | null;
  totals: CodexWorkspacesUsageTotals;
  costEstimate: CodexWorkspacesCostEstimate;
  sessionCount: number;
  latestActivity: string | null;
  topModel: string | null;
  topSessions: CodexWorkspacesSessionUsage[];
}

export interface CodexLocalProjectUsageSnapshot {
  updatedAt: string;
  historyDays: number;
  scopeSignature: string;
  indexedFileCount: number;
  skippedFileCount: number;
  total: CodexWorkspacesUsageTotals;
  projects: CodexWorkspacesProjectUsage[];
  daily: CodexWorkspacesDailyPoint[];
  sourceStatus: CodexWorkspacesSourceStatus;
}


export interface BootstrapState {
  contractVersion: string;
  providers: ProviderCatalogEntry[];
  settings: SettingsSnapshot;
}

// ── Provider usage snapshot types ────────────────────────────────────

export interface RateWindowSnapshot {
  usedPercent: number;
  remainingPercent: number;
  windowMinutes: number | null;
  resetsAt: string | null;
  resetDescription: string | null;
  isExhausted: boolean;
  isInformational?: boolean;
  reservePercent: number | null;
  reserveDescription: string | null;
  reserveWillLastToReset?: boolean;
  reserveEtaSeconds?: number | null;
}

export interface CostSnapshotBridge {
  used: number;
  limit: number | null;
  remaining: number | null;
  currencyCode: string;
  period: string;
  resetsAt: string | null;
  formattedUsed: string;
  formattedLimit: string | null;
  balance?: number | null;
  formattedBalance?: string | null;
}

export interface PaceSnapshot {
  stage: "on_track" | "slightly_ahead" | "ahead" | "far_ahead" | "slightly_behind" | "behind" | "far_behind";
  deltaPercent: number;
  willLastToReset: boolean;
  etaSeconds: number | null;
  expectedUsedPercent: number;
  actualUsedPercent: number;
}

export interface SessionEquivalentForecastSnapshot {
  estimatedWindowsToExhaustWeekly: number;
  windowsUntilReset: number;
  availableWindowsUntilReset: number;
  sampleCount: number;
  weeklyResetsAt: string;
  weeklyUsedPercent: number;
}

export interface ProviderUsageSnapshot {
  providerId: string;
  displayName: string;
  primary: RateWindowSnapshot;
  primaryLabel?: string;
  secondary: RateWindowSnapshot | null;
  secondaryLabel?: string;
  modelSpecific: RateWindowSnapshot | null;
  tertiary: RateWindowSnapshot | null;
  /** F5: duration-cadence label for tertiary ("monthly", "weekly" etc.) */
  tertiaryLabel?: string;
  extraRateWindows: Array<{
    id: string;
    title: string;
    window: RateWindowSnapshot;
  }>;
  cost: CostSnapshotBridge | null;
  planName: string | null;
  accountEmail: string | null;
  sourceLabel: string;
  updatedAt: string;
  error: string | null;
  pace: PaceSnapshot | null;
  accountOrganization: string | null;
  trayStatusLabel: string | null;
  fetchDurationMs?: number | null;
  wayfinderUsage?: WayfinderUsageSnapshot | null;
  sessionEquivalentForecast?: SessionEquivalentForecastSnapshot | null;
}

export interface WayfinderRouteSummary {
  name: string;
  requests: number;
  tokens: number;
  realized: number;
  baseline: number;
  saved: number;
}

export interface WayfinderUsageSnapshot {
  gatewayStatus: string;
  offline: boolean;
  dryRun: boolean;
  missingKeys: string[];
  modelCount: number;
  models: string[];
  requests: number;
  estimatedRequests: number;
  tokens: number;
  realized: number;
  baseline: number;
  saved: number;
  savedPercent: number;
  periodDays: number;
  unit: string;
  priced: boolean;
  routes: WayfinderRouteSummary[];
}

export interface RefreshCompletePayload {
  providerCount: number;
  errorCount: number;
}

export interface RefreshStartedPayload {
  providerIds: string[];
}

export interface CredentialStorageStatus {
  manualCookies: string;
  apiKeys: string;
  tokenAccounts: string;
}

// ── Update state types ───────────────────────────────────────────────

export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "error";

export interface UpdateStatePayload {
  status: UpdateStatus;
  version: string | null;
  error: string | null;
  progress: number | null;
  releaseUrl: string | null;
  canDownload: boolean;
  canApply: boolean;
  /** Unix-ms timestamp of the last completed update check, or `null`
   *  if the app has not checked during this session. */
  lastCheckedAt: number | null;
}

// ── Credential store types ───────────────────────────────────────────

export interface ApiKeyInfoBridge {
  providerId: string;
  provider: string;
  maskedKey: string;
  savedAt: string;
  label: string | null;
}

export interface ApiKeyProviderInfoBridge {
  id: string;
  displayName: string;
  envVar: string | null;
  help: string | null;
  dashboardUrl: string | null;
}

export interface CookieInfoBridge {
  providerId: string;
  provider: string;
  savedAt: string;
}

export interface DetectedBrowserBridge {
  browserType: string;
  displayName: string;
  profileCount: number;
}

export interface AppInfoBridge {
  name: string;
  version: string;
  buildNumber: string;
  updateChannel: string;
  tagline: string;
}

// ── Chart data types ─────────────────────────────────────────────────

export interface DailyCostPoint {
  date: string;
  value: number;
}

export interface ServiceUsagePoint {
  service: string;
  creditsUsed: number;
}

export interface DailyUsageBreakdown {
  day: string;
  services: ServiceUsagePoint[];
  totalCreditsUsed: number;
}

export interface ProviderLocalUsageSummary {
  todayCost: number | null;
  thirtyDayCost: number | null;
  thirtyDayTokens: number | null;
  latestTokens: number | null;
  topModel: string | null;
  estimateNote: string;
  tokenCostUpdatedAtMs: number;
}

export interface ProviderChartData {
  providerId: string;
  costHistory: DailyCostPoint[];
  creditsHistory: DailyCostPoint[];
  usageBreakdown: DailyUsageBreakdown[];
  localUsage: ProviderLocalUsageSummary | null;
}

// ── Token account types ──────────────────────────────────────────────

export interface TokenAccountSupportBridge {
  providerId: string;
  displayName: string;
  title: string;
  subtitle: string;
  placeholder: string;
}

export interface TokenAccountBridge {
  id: string;
  label: string;
  addedAt: string;
  lastUsed: string | null;
  isActive: boolean;
}

export interface ProviderTokenAccountsBridge {
  providerId: string;
  support: TokenAccountSupportBridge;
  accounts: TokenAccountBridge[];
  activeIndex: number;
}

// ── Phase 4 — provider ordering / cookie source / region ─────────────

export interface ProviderSummary {
  id: string;
  displayName: string;
  enabled: boolean;
  order: number;
}

// ── Phase 4 — credential detection ───────────────────────────────────

export interface GeminiCliStatus {
  signedIn: boolean;
  credentialsPath: string | null;
}

export interface VertexAiStatus {
  hasCredentials: boolean;
  credentialsPath: string | null;
}

export interface JetbrainsIde {
  id: string;
  displayName: string;
  path: string;
  detected: boolean;
}

export interface KiroStatus {
  available: boolean;
  hint: string | null;
}

// ── Phase 4 — session / environment ──────────────────────────────────

export interface WorkAreaRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

// ── Phase 5 — i18n ────────────────────────────────────────────────────

/** Snapshot returned by `get_locale_strings`. */
export interface LocaleStrings {
  language: Language;
  entries: Record<string, string>;
}

/** Payload emitted for `locale-changed`: the persisted language label. */
export type LocaleChangedPayload = Language;

// ── Phase 6b — provider detail pane ──────────────────────────────────

/** Aggregated per-provider payload powering the Settings detail pane. */
export interface ProviderDetail {
  id: string;
  displayName: string;
  enabled: boolean;

  // Identity
  email: string | null;
  plan: string | null;
  authType: string | null;
  sourceLabel: string | null;
  organization: string | null;
  lastUpdated: string | null;

  // Usage windows — mirror RateWindowSnapshot.
  session: RateWindowSnapshot | null;
  weekly: RateWindowSnapshot | null;
  modelSpecific: RateWindowSnapshot | null;
  tertiary: RateWindowSnapshot | null;
  extraRateWindows: Array<{
    id: string;
    title: string;
    window: RateWindowSnapshot;
  }>;

  cost: CostSnapshotBridge | null;
  pace: PaceSnapshot | null;

  lastError: string | null;

  dashboardUrl: string | null;
  statusPageUrl: string | null;
  buyCreditsUrl: string | null;

  hasSnapshot: boolean;

  /** Phase 6c — currently-persisted cookie source value ("auto" | "manual" | "off" | …).
   *  `null` for providers that do not expose a cookie-source picker. */
  cookieSource: string | null;
  /** Phase 6c — currently-persisted region value. `null` for non-regional providers. */
  region: string | null;
}

// ── Phase 6c — cookie-source & region pickers ────────────────────────

export interface CookieSourceOption {
  value: string;
  label: string;
  description?: string;
}

export interface RegionOption {
  value: string;
  label: string;
}

// ── Codex multi-account (ADR 0003) ───────────────────────────────────

export type CodexAccountSource = "ambient" | "managedByApp";

export interface CodexAccount {
  id: string;
  nickname: string | null;
  emailHint: string | null;
  authSubject: string | null;
  providerAccountId: string | null;
  codexHomePath: string;
  source: CodexAccountSource;
  createdAt: string;
  updatedAt: string;
  lastAuthenticatedAt: string | null;
}

export interface CodexUsageWindow {
  usedPercent: number;
  resetAt: string | null;
  limitWindowSeconds: number;
}

export interface CodexCreditsBalance {
  hasCredits: boolean;
  unlimited: boolean;
  balance: number | null;
}

export interface CodexAccountUsageSnapshot {
  email: string | null;
  providerAccountId: string | null;
  plan: string | null;
  allowed: boolean | null;
  limitReached: boolean | null;
  primaryWindow: CodexUsageWindow | null;
  secondaryWindow: CodexUsageWindow | null;
  credits: CodexCreditsBalance | null;
  updatedAt: string;
}

export interface CodexSwitchResult {
  materializedAccount: CodexAccount | null;
  backupPath: string | null;
  ambientAccount: CodexAccount | null;
  desktopSessionBackupPath: string | null;
  desktopSessionRestorePath: string | null;
  desktopSessionRestoreExists: boolean;
}

export interface CodexAccountsStateBridge {
  accounts: CodexAccount[];
  snapshots: Record<string, CodexAccountUsageSnapshot>;
}
