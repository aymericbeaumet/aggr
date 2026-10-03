import { describe, expect, it } from 'vitest';
import { displayRow, excerptText, isClientRow, resultRow } from './display';
import { hex, row } from './fixture';

describe('display decoding', () => {
  it('decodes the hex-encoded row once per result', () => {
    const result = { url: '/items/blog/2026/09/plain/', meta: { title: 'A plain article', aggr_display: hex(row) } };
    expect(displayRow(result)).toEqual(row);
    expect(displayRow(result)).toBe(displayRow(result));
  });

  it('rejects results without a well-formed row', () => {
    expect(displayRow({ url: '/x/', meta: {} })).toBeNull();
    expect(displayRow({ url: '/x/', meta: { aggr_display: 'zz' } })).toBeNull();
    expect(displayRow({ url: '/x/', meta: { aggr_display: hex({ title: 'flat', excerpt: 'old shape' }) } })).toBeNull();
    expect(isClientRow({ ...row, metadata: { ...row.metadata, discussions: 'no' } })).toBe(false);
    expect(isClientRow({ ...row, preview: { url: 'p.jpg', width: 1, height: 1, alt: null, color: null, placeholder: { data_url: 'data:' } } })).toBe(true);
  });

  it('prefers the highlighted excerpt as text, and the row summary when nothing matched', () => {
    const encoded = hex(row);
    const highlighted = resultRow({ url: '/x/', excerpt: 'the &lt;<mark>borrow</mark>&gt; checker &amp; more', meta: { aggr_display: encoded } });
    expect(highlighted?.excerpt).toBe('the <borrow> checker & more');
    expect(resultRow({ url: '/x/', excerpt: 'no marks here', meta: { aggr_display: encoded } })?.excerpt).toBe(row.excerpt);
    expect(excerptText('a&#39;b &#x41; <b>c</b>')).toBe("a'b A c");
  });
});
