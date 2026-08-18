import { useEffect, useState } from "react";
import type { LocaleKey } from "../../../../i18n/keys";
import {
  getProviderAccentColor,
  setProviderAccentColor,
} from "../../../../lib/tauri";
import { getProviderIcon } from "../../../../components/providers/providerIcons";

interface Props {
  providerId: string;
  t: (key: LocaleKey) => string;
}

/**
 * Per-provider accent color override (#2972): hex input, native color
 * picker, and a reset-to-shipped-color button. The override is persisted
 * via the `set_provider_accent_color` Tauri command and applied at runtime
 * through the `--provider-accent` CSS custom property.
 */
export function AccentColorSection({ providerId, t }: Props) {
  const [savedColor, setSavedColor] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const brandColor = getProviderIcon(providerId).brandColor;

  useEffect(() => {
    let cancelled = false;
    void getProviderAccentColor(providerId)
      .then((color) => {
        if (cancelled) return;
        setSavedColor(color);
        setInput(color ?? "");
      })
      .catch(() => {
        if (cancelled) return;
        setSavedColor(null);
        setInput("");
      });
    return () => {
      cancelled = true;
    };
  }, [providerId]);

  const effective = savedColor ?? brandColor;


  const handleSave = async (raw: string) => {
    setError(null);
    const trimmed = raw.trim();
    if (trimmed === "") {
      setSaving(true);
      try {
        await setProviderAccentColor(providerId, null);
        setSavedColor(null);
        setInput("");
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setSaving(false);
      }
      return;
    }
    const trimmedHex = trimmed.startsWith("#") ? trimmed.slice(1) : trimmed;
    if (trimmedHex.length !== 6 || !/^[0-9A-Fa-f]{6}$/.test(trimmedHex)) {
      setError(t("ProviderAccentColorInvalid"));
      return;
    }
    const normalized = `#${trimmedHex.toUpperCase()}`;
    setSaving(true);
    try {
      await setProviderAccentColor(providerId, normalized);
      setSavedColor(normalized);
      setInput(normalized);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleReset = async () => {
    setError(null);
    setSaving(true);
    try {
      await setProviderAccentColor(providerId, null);
      setSavedColor(null);
      setInput("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
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
          disabled={saving}
          onChange={(e) => {
            const value = e.target.value.toUpperCase();
            setInput(value);
            void handleSave(value);
          }}
        />
        <input
          type="text"
          className="accent-color-input"
          value={input}
          placeholder={brandColor}
          maxLength={7}
          spellCheck={false}
          disabled={saving}
          onChange={(e) => setInput(e.target.value)}
          onBlur={() => void handleSave(input)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              void handleSave(input);
            }
          }}
        />
        <button
          type="button"
          className="credential-btn credential-btn--secondary"
          disabled={saving || savedColor === null}
          onClick={() => void handleReset()}
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
