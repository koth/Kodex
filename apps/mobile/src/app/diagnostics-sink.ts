// expo-file-system-backed log sink for on-device diagnostics. Writes a
// rolling log file under the app cache directory; the Diagnostics screen
// reads it back. Append is implemented as read+rewrite (files stay small,
// capped by the in-memory ring at 500 lines).
import { File, Paths } from "expo-file-system";
import type { LogSink } from "../util/diagnostics";

const LOG_NAME = "diagnostics.log";
const MAX_CHARS = 200_000;

function logFile(): File {
  return new File(Paths.cache, LOG_NAME);
}

export class FileLogSink implements LogSink {
  async append(line: string): Promise<void> {
    const file = logFile();
    let existing = "";
    try {
      existing = file.textSync();
    } catch {
      // first write
    }
    let next = `${existing}${line}\n`;
    if (next.length > MAX_CHARS) {
      next = next.slice(next.length - MAX_CHARS);
    }
    file.write(next);
  }

  async read(): Promise<string> {
    try {
      return logFile().textSync();
    } catch {
      return "";
    }
  }

  async clear(): Promise<void> {
    try {
      logFile().write("");
    } catch {
      // ignore
    }
  }
}
