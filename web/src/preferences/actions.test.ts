import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreferenceActions } from "./actions";
import type { PreferencesService } from "./service";

afterEach(() => vi.unstubAllGlobals());

function fixture(navigator: object) {
  vi.stubGlobal("navigator", navigator);
  vi.stubGlobal("window", { location: { pathname: "/reader/preferences/" } });
  const field = { hidden: true, value: "", focus: vi.fn(), select: vi.fn() };
  const root = { querySelectorAll: () => [], querySelector: (selector: string) => selector === "#preferences-link" ? field : null };
  const service = {
    link: () => "https://example.test/preferences/#aggr-state=payload",
    showLink: vi.fn(), status: vi.fn(), cancelFileRead: vi.fn(),
  };
  const actions = createPreferenceActions(root as unknown as ParentNode, service as unknown as PreferencesService);
  return { field, service, actions };
}

describe("preference transfer actions", () => {
  it("selects a readable link when clipboard permission is denied", async () => {
    const f = fixture({ clipboard: { writeText: async () => { throw new Error("permission denied"); } } });
    await f.actions.run("copy");
    await Promise.resolve();
    expect(f.field.hidden).toBe(false);
    expect(f.field.value).toBe(f.service.link());
    expect(f.field.focus).toHaveBeenCalledOnce();
    expect(f.field.select).toHaveBeenCalledOnce();
    expect(f.service.status).toHaveBeenCalledWith("Copy the selected link. Clipboard access is unavailable.");
  });

  it("does not focus stale controls or overwrite status after page disposal", async () => {
    let reject!: (error: Error) => void;
    const f = fixture({ clipboard: { writeText: () => new Promise<void>((_, fail) => { reject = fail; }) } });
    const copying = f.actions.run("copy");
    f.actions.dispose();
    reject(new Error("denied"));
    await copying;
    expect(f.field.focus).not.toHaveBeenCalled();
    expect(f.service.status).not.toHaveBeenCalled();
    expect(f.service.cancelFileRead).toHaveBeenCalledOnce();
  });

  it("treats native share cancellation as cancellation", async () => {
    const f = fixture({ share: async () => { throw new DOMException("cancelled", "AbortError"); } });
    await f.actions.run("share");
    expect(f.actions.canShare).toBe(true);
    expect(f.service.status).not.toHaveBeenCalled();
  });
});
