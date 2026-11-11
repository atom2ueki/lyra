import { describe, expect, test } from "bun:test";
import { selectBackend } from "../src/pane/select";

describe("selectBackend", () => {
  test("explicit override wins", () => {
    const backend = selectBackend({ env: { LYRA_PANE_BACKEND: "headless", HERDR_ENV: "1" } });
    expect(backend.name).toBe("headless");
  });

  test("herdr env wins over ghostty", () => {
    const backend = selectBackend({ env: { HERDR_ENV: "1", TERM_PROGRAM: "ghostty" } });
    expect(backend.name).toBe("herdr");
  });

  test("ghostty env selects ghostty", () => {
    const backend = selectBackend({ env: { TERM_PROGRAM: "ghostty" } });
    expect(backend.name).toBe("ghostty");
  });

  test("no tty selects headless", () => {
    const backend = selectBackend({ env: {}, stdoutIsTTY: false });
    expect(backend.name).toBe("headless");
  });

  test("tmux and Zellij are explicit unimplemented errors", () => {
    expect(() => selectBackend({ env: { TMUX: "/tmp/tmux" } })).toThrow("tmux backend is not implemented");
    expect(() => selectBackend({ env: { ZELLIJ: "0" } })).toThrow("Zellij backend is not implemented");
  });
});
