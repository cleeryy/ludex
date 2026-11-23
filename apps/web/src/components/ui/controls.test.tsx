// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import {
  Badge,
  Button,
  Card,
  Input,
  Led,
  SegmentedGauge,
  TelemetryStrip,
} from "@/components/ui/controls";
import {
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "@/components/ui/table";
import { PublicPage } from "@/components/public-page";

afterEach(cleanup);

describe("design system issue 333 core primitives", () => {
  it("renders Button variants and sizes with semantic radius and active tactile scale", () => {
    const { rerender } = render(<Button variant="primary" size="sm">Launch</Button>);
    const btn = screen.getByRole("button", { name: "Launch" });
    expect(btn.className).toContain("bg-signal");
    expect(btn.className).toContain("h-[30px]");
    expect(btn.className).toContain("rounded-[var(--r-sm)]");
    expect(btn.className).toContain("active:scale-[0.98]");

    rerender(<Button variant="ink" size="lg">Install</Button>);
    expect(screen.getByRole("button", { name: "Install" }).className).toContain("bg-ink");
    expect(screen.getByRole("button", { name: "Install" }).className).toContain("h-10");
  });

  it("renders inputs on bg-deep with 1px line-2 border", () => {
    render(<Input aria-label="Search" />);
    const input = screen.getByLabelText("Search");
    expect(input.className).toContain("bg-bg-deep");
    expect(input.className).toContain("border-line-2");
    expect(input.className).toContain("rounded-[var(--r-sm)]");
  });

  it("renders 16-segment dual-color SegmentedGauge with signal and info bands", () => {
    render(<SegmentedGauge value={50} active label="Download progress" />);
    const bar = screen.getByRole("progressbar", { name: "Download progress" });
    expect(bar.getAttribute("aria-valuenow")).toBe("50");
    const cells = Array.from(bar.querySelectorAll("span"));
    expect(cells).toHaveLength(16);
    expect(cells.filter((el) => el.className.includes("bg-signal"))).toHaveLength(8);
    expect(cells.filter((el) => el.className.includes("bg-info"))).toHaveLength(2);
    expect(cells.filter((el) => el.className.includes("bg-line"))).toHaveLength(6);
  });

  it("renders 4-column TelemetryStrip with tabular numerals", () => {
    render(
      <TelemetryStrip
        label="Transfer telemetry"
        items={[
          { label: "Active", value: "2", tone: "signal" },
          { label: "Throughput", value: "48 MB/s", tone: "info" },
          { label: "Verified", value: "100%", tone: "ok" },
          { label: "Disk", value: "1.2 TB" },
        ]}
      />,
    );
    expect(screen.getByText("Throughput")).toBeTruthy();
    expect(screen.getByText("48 MB/s").className).toContain("num");
    expect(screen.getByText("48 MB/s").className).toContain("text-info");
  });

  it("renders square 7px Led, Badge tones, Card, and Swiss Table", () => {
    const { container } = render(
      <Card>
        <Badge tone="info" led>Streaming</Badge>
        <Led tone="ok" />
        <TableContainer>
          <Table>
            <TableHead>
              <TableRow>
                <TableHeaderCell>Title</TableHeaderCell>
                <TableHeaderCell numeric>Size</TableHeaderCell>
              </TableRow>
            </TableHead>
            <TableBody>
              <TableRow>
                <TableCell>Outer Wilds</TableCell>
                <TableCell numeric>8.4 GB</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </TableContainer>
      </Card>,
    );
    expect(container.querySelector(".size-\\[7px\\]")).toBeTruthy();
    expect(screen.getByText("Title").className).not.toContain("font-mono");
    expect(screen.getByText("8.4 GB").className).toContain("num");
  });

  it("renders the 3px ARC hardware stripe at the top of PublicPage", () => {
    const { container } = render(<PublicPage><p>Hello</p></PublicPage>);
    expect(container.querySelector(".arc-stripe")).toBeTruthy();
  });
});
