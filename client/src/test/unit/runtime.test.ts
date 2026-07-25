import * as assert from "assert";
import * as path from "path";
import {
  bundledRuntimeFile,
  normalizeRuntimeVersion,
  resolveBrExecutable,
  runtimeLabel,
} from "../../runtime";

const LEXI = path.join("/ext", "Lexi");

// Paths are joined with the host path module, so a linux case on a Windows box
// still yields backslashes — assert on basename/dirname, never a full literal.
function resolve(opts: {
  version?: any;
  platform?: NodeJS.Platform;
  configuredExe?: string;
  exists?: (p: string) => boolean;
}) {
  return resolveBrExecutable({
    lexiPath: LEXI,
    version: opts.version ?? "4.3",
    platform: opts.platform ?? "win32",
    configuredExe: opts.configuredExe ?? "",
    exists: opts.exists ?? (() => true),
  });
}

suite("resolveBrExecutable", () => {
  test("win32 + 4.3 uses the bundled brnative.exe", () => {
    const r = resolve({ platform: "win32", version: "4.3" });
    assert.strictEqual(path.basename(r.path), "brnative.exe");
    assert.strictEqual(path.dirname(r.path), LEXI);
    assert.strictEqual(r.bundled, true);
    assert.strictEqual(r.version, "4.3");
    assert.strictEqual(r.error, undefined);
  });

  test("win32 + 4.2 uses the bundled brnative.42.exe", () => {
    const r = resolve({ platform: "win32", version: "4.2" });
    assert.strictEqual(path.basename(r.path), "brnative.42.exe");
    assert.strictEqual(r.bundled, true);
    assert.strictEqual(r.version, "4.2");
    assert.strictEqual(r.error, undefined);
  });

  test("linux + 4.3 uses the bundled brlinux", () => {
    const r = resolve({ platform: "linux", version: "4.3" });
    assert.strictEqual(path.basename(r.path), "brlinux");
    assert.strictEqual(r.bundled, true);
    assert.strictEqual(r.error, undefined);
  });

  test("linux + 4.2 errors instead of claiming a runtime", () => {
    const r = resolve({ platform: "linux", version: "4.2" });
    assert.strictEqual(r.path, "");
    assert.ok(r.error);
    assert.ok(r.error!.includes("Windows"));
  });

  test("br.executable wins over the version setting", () => {
    const custom = path.join("/opt", "br", "brnative.exe");
    const r = resolve({ platform: "win32", version: "4.2", configuredExe: custom });
    assert.strictEqual(r.path, custom);
    assert.strictEqual(r.bundled, false);
    assert.strictEqual(r.error, undefined);
  });

  test("br.executable wins on linux too, including for 4.2", () => {
    const custom = path.join("/opt", "br", "br42");
    const r = resolve({ platform: "linux", version: "4.2", configuredExe: custom });
    assert.strictEqual(r.path, custom);
    assert.strictEqual(r.bundled, false);
    assert.strictEqual(r.error, undefined);
  });

  test("missing br.executable reports the configured path", () => {
    const custom = path.join("/opt", "br", "gone.exe");
    const r = resolve({ configuredExe: custom, exists: () => false });
    assert.ok(r.error);
    assert.ok(r.error!.includes(custom));
  });

  test("missing bundled runtime names the version", () => {
    const r = resolve({ platform: "win32", version: "4.2", exists: () => false });
    assert.ok(r.error);
    assert.ok(r.error!.includes("4.2"));
    assert.ok(r.error!.includes("brnative.42.exe"));
  });

  test("unrecognized version falls back to 4.3", () => {
    const r = resolve({ platform: "win32", version: "4.1" });
    assert.strictEqual(r.version, "4.3");
    assert.strictEqual(path.basename(r.path), "brnative.exe");
  });
});

suite("runtime helpers", () => {
  test("normalizeRuntimeVersion accepts the two supported versions", () => {
    assert.strictEqual(normalizeRuntimeVersion("4.2"), "4.2");
    assert.strictEqual(normalizeRuntimeVersion("4.3"), "4.3");
  });

  test("normalizeRuntimeVersion falls back for anything else", () => {
    assert.strictEqual(normalizeRuntimeVersion(undefined), "4.3");
    assert.strictEqual(normalizeRuntimeVersion(""), "4.3");
    assert.strictEqual(normalizeRuntimeVersion("4.1"), "4.3");
  });

  test("bundledRuntimeFile has no 4.2 build for non-Windows", () => {
    assert.strictEqual(bundledRuntimeFile("4.2", "win32"), "brnative.42.exe");
    assert.strictEqual(bundledRuntimeFile("4.3", "win32"), "brnative.exe");
    assert.strictEqual(bundledRuntimeFile("4.3", "linux"), "brlinux");
    assert.strictEqual(bundledRuntimeFile("4.2", "linux"), undefined);
  });

  test("runtimeLabel does not claim a version for a custom executable", () => {
    assert.strictEqual(runtimeLabel({ path: "x", version: "4.3", bundled: true }), "BR 4.3");
    assert.strictEqual(runtimeLabel({ path: "x", version: "4.2", bundled: true }), "BR 4.2");
    assert.strictEqual(runtimeLabel({ path: "x", version: "4.3", bundled: false }), "BR (custom)");
  });
});
