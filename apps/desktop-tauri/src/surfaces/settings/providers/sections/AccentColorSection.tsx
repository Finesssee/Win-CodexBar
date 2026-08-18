import { useState } from "react";
import type { LocaleKey } from "../../../../i18n/keys";
import type { SettingsUpdate } from "../../../../types/bridge";
import { getProviderIcon } from "../../../../components/providers/providerIcons";

interface Props {
  providerId: string;
  accentColor: string | null;
  t: (key: LocaleKey) => string;
  onChange: (patch: SettingsUpdate) => void;
}

/**
 * Per-provider accent color override (#2972): hex input, native color
 * picker, and a reset-to-shipped-color button. The override is persisted
 * via the standard settings-update flow (onSettingsChange).
 */
export function AccentColorSection({
  providerId,
  accentColor,
  t,
  onChange,
}: Props) {
  const [input, setInput] = useState(accentColor ?? "");
  const [error, setError] = useState<string | null>(null);

  const brandColor = getProviderIcon(providerId).brandColor;
  const effective = accentColor ?? brandColor;

  const handleSave = (raw: string) => {
    setError(null);
    const trimmed = raw.trim();
    if (trimmed === "") {
      onChange({ providerAccentColors: { [providerId]: null } });
      setInput("");
      return;
    }
    const trimmedHex = trimmed.startsWith("#") ? trimmed.slice(1) : trimmed;
    if (trimmedHex.length !== 6 || !/^[0-9A-Fa-f]{6}$/.test(trimmedHex)) {
      setError(t("ProviderAccentColorInvalid"));
      return;
    }
    const normalized = `#${trimmedHex.toUpperCase()}`;
    onChange({ providerAccentColors: { [providerId]: normalized } });
    setInput(normalized);
  };

  const handleReset = () => {
    setError(null);
    onChange({ providerAccentColors: { [providerId]: null } });
    setInput("");
  };

  return (
    <section className="provider-detail-section provider-detail-accent-color">
      <h4>{t("ProviderAccentColor")}</h4>
      <p className="provider-detail-section__helper">
        {t("ProviderAccentColorHelper")}
      </p>
      <div className="accent-color-row">
        <input
          type="color"
          className="accent-color-picker"
          value={effective}
          aria-label={t("ProviderAccentColor")}
          onChange={(e) => {
            const value = e.target.value.toUpperCase();
            setInput(value);
            handleSave(value);
          }}
        />
        <input
          type="text"
          className="accent-color-input"
          value={input}
          placeholder={brandColor}
          maxLength={7}
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          onBlur={() => handleSave(input)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              handleSave(input);
            }
          }}
        />
        <button
          type="button"
          className="credential-btn credential-btn--secondary"
          disabled={accentColor === null}
          onClick={handleReset}
          title={t("ProviderAccentColorReset")}
        >
          {t("ProviderAccentColorReset")}
        </button>
      </div>
      <div className="accent-color-swatch-row">
        <span className="accent-color-swatch-label">
          {t("ProviderAccentColor")}
        </span>
        <span
          className="accent-color-swatch"
          style={{ background: effective }}
        />
        <span className="accent-color-swatch-value">{effective}</span>
      </div>
      {error && <p className="settings-section__error">{error}</p>}
    </section>
  );
}
