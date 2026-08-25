import type { ProviderDetail } from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";
import { describeProviderState } from "../../../../lib/providerState";

interface Props {
  detail: ProviderDetail;
  rawError: string;
  t: (key: LocaleKey) => string;
}

export function ProviderIssueNotice({ detail, rawError, t }: Props) {
  const state = describeProviderState(rawError);
  const title = `${detail.displayName}: ${t(state.labelKey)}`;

  return (
    <div className="provider-detail-error" role="status">
      <div className="provider-detail-error__header">
        <strong>{title}</strong>
      </div>
      <p>{t("ProviderIssuePrivacySafeDetail")}</p>
    </div>
  );
}
