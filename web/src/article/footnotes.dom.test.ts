import { afterEach, describe, expect, it } from 'vitest';
import { installFootnotes, pairFootnote } from './footnotes';

// The markup `src/content/render.rs` writes: comrak's footnote with the margin note the build
// places beside its reference, and the notes list at the end.
const BODY = `<div class="body has-margin-notes">
<p>Text<sup class="footnote-ref"><a href="#fn-1" id="fnref-1" data-footnote-ref aria-describedby="fnref-1-note">1</a></sup><aside class="margin-note footnote-margin-note" role="note" aria-label="Note 1" id="fnref-1-note"><p><span class="margin-note-number">1. </span>First note.</p></aside> and more<sup class="footnote-ref"><a href="#fn-2" id="fnref-2" data-footnote-ref>2</a></sup>.</p>
<p>A citation<sup class="citation-ref"><a href="#ref-3" id="cite-3">[3]</a></sup>.</p>
<section class="footnotes" data-footnotes><ol>
<li id="fn-1"><p>First note. <a href="#fnref-1" class="footnote-backref" data-footnote-backref>↩</a></p></li>
<li id="fn-2"><p>Second note. <a href="#fnref-2" class="footnote-backref" data-footnote-backref>↩</a></p></li>
</ol></section>
<p id="ref-3">Reference three.</p>
</div>`;

function region(): HTMLElement {
  const wrapper = document.createElement('div');
  wrapper.setAttribute('data-article-content', '');
  wrapper.innerHTML = BODY;
  document.body.append(wrapper);
  return wrapper;
}

const active = (root: ParentNode) => [...root.querySelectorAll('[data-footnote-active]')].map((node) => node.id || node.className);

afterEach(() => {
  document.body.innerHTML = '';
  location.hash = '';
});

describe('pairFootnote', () => {
  it('marks a followed note and the margin note standing in for it', () => {
    const root = region();
    pairFootnote(root, 'fn-1');
    expect(active(root)).toEqual(['fnref-1-note', 'fn-1']);
  });

  it('marks the reference when the way back is followed', () => {
    const root = region();
    pairFootnote(root, 'fnref-2');
    expect(active(root)).toEqual(['footnote-ref']);
    expect(root.querySelector('#fnref-2')?.parentElement?.hasAttribute('data-footnote-active')).toBe(true);
  });

  it('marks a bracketed citation the same way', () => {
    const root = region();
    pairFootnote(root, 'cite-3');
    expect(active(root)).toEqual(['citation-ref']);
  });

  it('clears earlier marks, and marks nothing for an unrelated or missing fragment', () => {
    const root = region();
    pairFootnote(root, 'fn-1');
    pairFootnote(root, 'ref-3');
    expect(active(root)).toEqual([]);
    pairFootnote(root, 'fn-2');
    pairFootnote(root, 'nowhere');
    expect(active(root)).toEqual([]);
    pairFootnote(root, '');
    expect(active(root)).toEqual([]);
  });

  it('ignores an id outside the region', () => {
    const root = region();
    const outside = document.createElement('p');
    outside.id = 'fn-9';
    document.body.append(outside);
    pairFootnote(root, 'fn-9');
    expect(active(root)).toEqual([]);
  });
});

describe('installFootnotes', () => {
  it('pairs the fragment on arrival and again on every hash change, until the page is left', () => {
    location.hash = '#fn-2';
    const root = region();
    const controller = new AbortController();
    installFootnotes(root, controller.signal);
    expect(active(root)).toEqual(['fn-2']);
    location.hash = '#fnref-1';
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    expect(active(root)).toEqual(['footnote-ref']);
    controller.abort();
    location.hash = '#fn-1';
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    expect(active(root)).toEqual(['footnote-ref']);
  });

  it('leaves a reference link alone when its margin note is not on screen', () => {
    const root = region();
    installFootnotes(root, new AbortController().signal);
    const link = root.querySelector<HTMLAnchorElement>('#fnref-1');
    const event = new MouseEvent('click', { bubbles: true, cancelable: true });
    link?.dispatchEvent(event);
    // jsdom lays nothing out, so the note has no client rects: the link keeps its jump.
    expect(event.defaultPrevented).toBe(false);
  });
});
