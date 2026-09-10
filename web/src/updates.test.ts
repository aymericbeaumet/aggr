import { afterEach, describe, expect, it, vi } from 'vitest';
import { createUpdates, connectionPresentation } from './updates';

afterEach(() => vi.useRealTimers());
describe('independent application and content updates', () => {
  it('refreshes content while an application release waits for the reader', () => {
    const updates = createUpdates({ appVersion: 'app-1', contentVersion: 'content-1' }, true);
    expect(updates.receive({ app_version: 'app-1', content_version: 'content-2' })).toEqual({ appChanged: false, contentChanged: true });
    expect(connectionPresentation(updates.snapshot()).visible).toBe(false);
    expect(updates.receive({ app_version: 'app-2', content_version: 'content-2' }).appChanged).toBe(true);
    expect(updates.receive({ app_version: 'app-2', content_version: 'content-3' })).toEqual({ appChanged: false, contentChanged: true });
    expect(updates.snapshot().contentVersion).toBe('content-3');
    expect(connectionPresentation(updates.snapshot()).refresh).toBe(true);
  });

  it('ignores incomplete deployments and gives offline status priority over refresh', () => {
    const updates = createUpdates({ appVersion: 'a', contentVersion: 'c' }, true);
    updates.receive({ app_version: 'b' });
    expect(updates.snapshot().availableAppVersion).toBe('a');
    updates.receive({ app_version: 'b', content_version: 'd' });
    updates.setOnline(false);
    expect(connectionPresentation(updates.snapshot())).toMatchObject({ visible: true, retry: true, refresh: false });
    updates.setOnline(true);
    expect(connectionPresentation(updates.snapshot()).refresh).toBe(true);
  });

  it('temporary notices cannot hide a pending release when their timer finishes', () => {
    vi.useFakeTimers();
    const updates = createUpdates({ appVersion: 'a', contentVersion: 'c' }, true);
    updates.notice('Checking…', false, true);
    updates.receive({ app_version: 'b', content_version: 'd' });
    vi.advanceTimersByTime(2300);
    expect(connectionPresentation(updates.snapshot()).refresh).toBe(true);
    updates.dispose();
  });
});
