// On-device diagnostic log: ring buffer + optional file persistence, so
// pairing/connection issues on a physical phone can be read from the app's
// own Diagnostics screen without adb/Xcode. Keep it dependency-light: the
// file sink is injected (expo-file-system in the app, in-memory in tests).

export interface LogSink {
  append(line: string): Promise<void>;
  read(): Promise<string>;
  clear(): Promise<void>;
}

const MAX_LINES = 500;

class DiagnosticsLog {
  private lines: string[] = [];
  private sink: LogSink | null = null;

  setSink(sink: LogSink | null): void {
    this.sink = sink;
  }

  /** Append a line. Also mirrors to console.log for dev (Expo terminal). */
  log(tag: string, message: string): void {
    const line = `${new Date().toISOString()} [${tag}] ${message}`;
    this.lines.push(line);
    if (this.lines.length > MAX_LINES) {
      this.lines.splice(0, this.lines.length - MAX_LINES);
    }
    console.log(line);
    this.sink?.append(line).catch(() => {});
  }

  async readAll(): Promise<string> {
    if (this.sink) {
      const fromDisk = await this.sink.read().catch(() => "");
      if (fromDisk) return fromDisk;
    }
    return this.lines.join("\n");
  }

  async clear(): Promise<void> {
    this.lines = [];
    await this.sink?.clear().catch(() => {});
  }
}

export const diagnostics = new DiagnosticsLog();
