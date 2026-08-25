import type { ProviderDetail } from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";

interface Props {
  provider: ProviderDetail;
  busy: boolean;
  onRefresh: () => void;
  onSwitchAccount: () => void;
  onOpenDashboard: () => void;
  onOpenStatusPage: () => void;
  onBuyCredits: () => void;
  t: (key: LocaleKey) => string;
}

/**
 * Quick-action toolbar. Mirrors the six buttons in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 * Buttons that have no backing URL on the provider are omitted entirely
 * (egui parity: the button only renders when the action is meaningful).
 */
export function QuickActionsSection({
  provider,
  busy,
  onRefresh,
  onSwitchAccount,
  onOpenDashboard,
  onOpenStatusPage,
  onBuyCredits,
  t,
}: Props) {
  return (
    <section className="provider-detail-section">
      <h4>{t("QuickActions")}</h4>
      <div className="provider-detail-actions">
        <button
          type="button"
          className="btn btn--ghost"
          onClick={onRefresh}
          disabled={busy}
        >
          {t("ActionRefresh")}
        </button>
        {provider.dashboardUrl && (
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onSwitchAccount}
            disabled={busy}
          >
            {t("ActionSwitchAccount")}
          </button>
        )}
        {provider.dashboardUrl && (
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onOpenDashboard}
          >
            {t("ActionUsageDashboard")}
          </button>
        )}
        {provider.statusPageUrl && (
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onOpenStatusPage}
          >
            {t("ActionStatusPage")}
          </button>
        )}
        {provider.buyCreditsUrl && (
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onBuyCredits}
          >
            {t("ActionBuyCredits")}
          </button>
        )}
      </div>
    </section>
  );
}
