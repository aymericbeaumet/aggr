import { describe, expect, it } from 'vitest';
import { controlOwnsArrows, editing } from './keys';

function mount(html: string): HTMLElement {
  const holder = document.createElement('div');
  holder.innerHTML = html;
  document.body.append(holder);
  const node = holder.firstElementChild;
  if (!(node instanceof HTMLElement)) throw new Error('missing element');
  return node;
}

describe('editing', () => {
  it('is text entry only', () => {
    expect(editing(mount('<textarea></textarea>'))).toBe(true);
    expect(editing(mount('<input type="search" role="combobox">'))).toBe(true);
    expect(editing(mount('<input type="email">'))).toBe(true);
    expect(editing(mount('<input>'))).toBe(true);
    expect(editing(mount('<div role="searchbox"></div>'))).toBe(true);
    const editable = mount('<div contenteditable="true"><span>word</span></div>');
    expect(editing(editable.querySelector('span'))).toBe(true);
    expect(editing(mount('<div contenteditable="false"></div>'))).toBe(false);
    expect(editing(mount('<select><option>1×</option></select>'))).toBe(false);
    expect(editing(mount('<input type="range">'))).toBe(false);
    expect(editing(mount('<input type="checkbox">'))).toBe(false);
    expect(editing(mount('<video></video>'))).toBe(false);
    expect(editing(null)).toBe(false);
  });
});

describe('controlOwnsArrows', () => {
  it('keeps arrows on sliders, selects, and native playback', () => {
    expect(controlOwnsArrows(mount('<input type="range">'))).toBe(true);
    expect(controlOwnsArrows(mount('<select><option>1×</option></select>'))).toBe(true);
    expect(controlOwnsArrows(mount('<video></video>'))).toBe(true);
    expect(controlOwnsArrows(mount('<audio></audio>'))).toBe(true);
    expect(controlOwnsArrows(mount('<input type="search">'))).toBe(false);
    expect(controlOwnsArrows(mount('<textarea></textarea>'))).toBe(false);
  });
});
