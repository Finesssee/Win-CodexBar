import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { LineChart } from "./LineChart";

describe("LineChart unknown values", () => {
  it("renders gaps for unknown values while preserving known zero", () => {
    const { container } = render(
      <LineChart
        data={[
          { label: "2026-09-01", value: 1 },
          { label: "2026-09-02", value: 0 },
          { label: "2026-09-03", value: null },
          { label: "2026-09-04", value: 2 },
          { label: "2026-09-05", value: 3 },
        ]}
        ariaLabel="credits history"
        animations={false}
      />,
    );

    expect(container.querySelectorAll(".chart__point")).toHaveLength(4);
    expect(container.querySelectorAll(".chart__line")).toHaveLength(2);
    expect(container).toHaveTextContent("2026-09-02: 0.00");
    expect(container).not.toHaveTextContent("2026-09-03: 0.00");
  });
});