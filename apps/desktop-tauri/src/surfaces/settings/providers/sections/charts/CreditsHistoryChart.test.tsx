import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CreditsHistoryChart } from "./CreditsHistoryChart";

describe("CreditsHistoryChart", () => {
  it("preserves unknown history values instead of fabricating zero", () => {
    const { container } = render(
      <CreditsHistoryChart
        data={[
          { date: "2026-09-01", value: 5 },
          { date: "2026-09-02", value: null },
          { date: "2026-09-03", value: 0 },
        ]}
        title="Credits"
        ariaLabel="credits history"
        providerId="codex"
        animations={false}
        emptyMessage="No history"
      />,
    );

    expect(container.querySelectorAll(".chart__point")).toHaveLength(2);
    expect(container).toHaveTextContent("2026-09-03: 0.0");
    expect(container).not.toHaveTextContent("2026-09-02: 0.0");
  });
});