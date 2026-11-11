import { beforeEach, describe, expect, it, testTools } from "../../test/browser";
import { search, type SearchDriver } from '../state/search.svelte';
import { completionKeydown } from './completion-keys';

let field: HTMLInputElement;

const driver = (): SearchDriver => ({
  arrive: testTools.fn(), leave: testTools.fn(), changed: testTools.fn(), moved: testTools.fn(), focus: testTools.fn(), blur: testTools.fn(),
  choose: testTools.fn(), highlight: testTools.fn(), close: testTools.fn(() => true), submit: testTools.fn(), clear: testTools.fn(),
  turn: testTools.fn(), refresh: testTools.fn(),
});

function press(key: string, shiftKey = false) {
  const event = new KeyboardEvent('keydown', { key, shiftKey, bubbles: true, cancelable: true });
  let handled = false;
  field.addEventListener('keydown', event => { handled = completionKeydown(event); }, { once: true });
  field.dispatchEvent(event);
  return { handled, event };
}

beforeEach(() => {
  document.body.innerHTML = '<input type="search">';
  field = document.querySelector('input')!;
  field.focus();
  search.text = 'source:';
  search.open = true;
  search.highlighted = 0;
  search.driver = null;
  search.completions = ['one', 'two'].map(id => ({ id, label: id, detail: '', insert: id, start: 0, end: 7 }));
});

describe('suggestion keyboard behavior', () => {
  it('Escape closes suggestions without clearing the query or leaving the field', () => {
    const { handled, event } = press('Escape');
    expect(handled && event.defaultPrevented).toBe(true);
    expect(search.open).toBe(false);
    expect(search.text).toBe('source:');
    expect(document.activeElement).toBe(field);
  });

  it('Escape leaves the field once suggestions are closed and preserves the query', () => {
    search.open = false;
    press('Escape');
    expect(document.activeElement).not.toBe(field);
    expect(search.text).toBe('source:');
  });

  it('arrows wrap the highlight before the search driver loads', () => {
    expect(press('ArrowUp').event.defaultPrevented).toBe(true);
    expect(search.highlighted).toBe(1);
    press('ArrowDown');
    expect(search.highlighted).toBe(0);
  });

  it.each(['Enter', 'Tab'])('%s chooses the highlighted suggestion without submitting', key => {
    const loaded = driver();
    search.driver = loaded;
    search.highlighted = 1;
    expect(press(key).handled).toBe(true);
    expect(loaded.choose).toHaveBeenCalledWith(search.completions[1]);
    expect(loaded.submit).not.toHaveBeenCalled();
    expect(loaded.clear).not.toHaveBeenCalled();
  });

  it('delegates highlighting and dismissal to a loaded search driver', () => {
    const loaded = driver();
    search.driver = loaded;
    press('ArrowUp');
    expect(loaded.highlight).toHaveBeenCalledWith(1);
    press('Escape');
    expect(loaded.close).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(field);
  });

  it('leaves Shift-Tab and unrelated keys to the field', () => {
    expect(press('Tab', true).handled).toBe(false);
    expect(press('a').event.defaultPrevented).toBe(false);
    search.open = false;
    expect(press('Enter').handled).toBe(false);
  });
});
