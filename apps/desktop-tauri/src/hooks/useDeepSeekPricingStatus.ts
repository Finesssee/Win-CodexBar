import { useEffect } from "react";
import { getDeepSeekPricingStatus } from "../lib/tauri";
import type { DeepSeekPricingStatus } from "../types/bridge";

export const DEEPSEEK_PRICING_EVENT = "codexbar:deepseek-pricing";

export function useDeepSeekPricingStatus(): void {
  useEffect(() => {
    let cancelled = false;
    const poll = () => {
      getDeepSeekPricingStatus()
        .then((status) => {
          if (!cancelled && status) {
            window.dispatchEvent(
              new CustomEvent<DeepSeekPricingStatus>(DEEPSEEK_PRICING_EVENT, {
                detail: status,
              }),
            );
          }
        })
        .catch(() => {});
    };
    poll();
    const timer = window.setInterval(poll, 60_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);
}
