import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderDetail } from "../../../../types/bridge";
import { ProviderIssueNotice } from "./ProviderIssueNotice";

const detail = {
  id: "cursor",
  displayName: "Cursor",
} as ProviderDetail;

describe("ProviderIssueNotice", () => {
  it("renders a categorized notice without rendering the raw diagnostic", () => {
    const raw = "cookie=super-secret; authentication required at https://private.example.test";
    const t = vi.fn((key: string) => ({
      ProviderIssueAuthRequired: "Sign-in required",
      ProviderIssuePrivacySafeDetail: "Details are hidden here to protect account data.",
    })[key] ?? key);

    render(<ProviderIssueNotice detail={detail} rawError={raw} t={t} />);

    expect(screen.getByRole("status")).toHaveTextContent("Cursor: Sign-in required");
    expect(screen.getByRole("status")).toHaveTextContent(
      "Details are hidden here to protect account data.",
    );
    expect(screen.queryByText(/super-secret|private\.example\.test/i)).toBeNull();
  });
});
