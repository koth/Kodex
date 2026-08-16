import { describe, it, expect } from "vitest";
import { diagnostics, type LogSink } from "../util/diagnostics";

class MemSink implements LogSink {
  lines: string[] = [];
  async append(line: string): Promise<void> {
    this.lines.push(line);
  }
  async read(): Promise<string> {
    return this.lines.join("\n");
  }
  async clear(): Promise<void> {
    this.lines = [];
  }
}

describe("diagnostics log", () => {
  it("buffers lines with a timestamp and tag, mirroring to the sink", async () => {
    const sink = new MemSink();
    diagnostics.setSink(sink);
    diagnostics.log("pairing", "resume sent token=abc…");
    diagnostics.log("conn", "recv plain");
    const text = await diagnostics.readAll();
    expect(text).toContain("[pairing] resume sent token=abc…");
    expect(text).toContain("[conn] recv plain");
    await diagnostics.clear();
    expect(await diagnostics.readAll()).toBe("");
    diagnostics.setSink(null); // detach for other tests
  });
});
