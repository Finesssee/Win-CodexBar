import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  setProviderUsageSource: vi.fn(),
}));

vi.mock("../../../../lib/tauri", () => tauriMocks);

import { UsageSourceSection } from "./UsageSourceSection";

describe("UsageSourceSection", () => {
  it("offers Bailian Auto, CLI, and Web and persists explicit CLI selection", async () => {
    const onChanged = vi.fn();
    tauriMocks.setProviderUsageSource.mockResolvedValue(undefined);

    render(
      <UsageSourceSection
        providerId="alibabatokenplan"
        currentValue="auto"
        t={(key) => key}
        onChanged={onChanged}
      />,
    );

    expect(screen.getByRole("radio", { name: "Auto" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Bailian CLI" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Browser cookies" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "Bailian CLI" }));

    await waitFor(() => {
      expect(tauriMocks.setProviderUsageSource).toHaveBeenCalledWith("alibabatokenplan", "cli");
      expect(onChanged).toHaveBeenCalledTimes(1);
    });
  });

  it("does not render for unrelated providers", () => {
    const { container } = render(
      <UsageSourceSection providerId="codex" currentValue="auto" t={(key) => key} onChanged={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
