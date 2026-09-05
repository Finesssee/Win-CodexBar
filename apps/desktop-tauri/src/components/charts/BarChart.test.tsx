import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BarChart } from "./BarChart";

describe("BarChart calendar slots", () => {
  it("keeps unknown and known-zero slots distinct", () => {
    const { container } = render(
      <BarChart
        data={[
          { label: "unknown", value: null },
          { label: "zero", value: 0 },
          { label: "known", value: 2 },
        ]}
        ariaLabel="history"
        animations={false}
      />,
    );
    const bars = container.querySelectorAll(".chart__bar");
    expect(bars).toHaveLength(3);
    expect(bars[0]).toHaveAttribute("opacity", "0");
    expect(bars[1]).toHaveAttribute("opacity", "0.25");
    expect(container).toHaveTextContent("unknown: Unknown");
    expect(container).toHaveTextContent("zero: 0.00");
  });
});