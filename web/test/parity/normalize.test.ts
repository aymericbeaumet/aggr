import { describe, expect, it } from 'vitest';
import { normalize } from './normalize';

describe('normalize', () => {
  it('ignores comments, attribute order, entity spelling and void syntax', () => {
    expect(normalize('<!--[--><a href="x" hidden>a &amp; b</a><img src="y"/><!--]-->')).toBe(
      normalize('<a hidden="" href="x">a &#38; b</a><img src="y">'),
    );
  });

  it('keeps one space between inline siblings and none around blocks', () => {
    expect(normalize('<div>\n  <span>1.</span>\n  <a>t</a>\n</div>')).toBe(
      normalize('<div><span>1.</span> <a>t</a></div>'),
    );
    expect(normalize('<div><span>1.</span><a>t</a></div>')).not.toBe(
      normalize('<div><span>1.</span> <a>t</a></div>'),
    );
    expect(normalize('<li>\n  <a></a>\n  <div>x</div>\n  <data value="v"></data>  </li>')).toBe(
      normalize('<li><a></a><div>x</div><data value="v"></data></li>'),
    );
  });

  it('trims text at the start and end of an element', () => {
    expect(normalize('<h1>\n   Blog\n<a>(p)</a>  </h1>')).toBe(normalize('<h1>Blog <a>(p)</a></h1>'));
    expect(normalize('<a>\n<svg><path d="M1"/></svg>\n<span>feed</span>\n</a>')).toBe(
      normalize('<a><svg><path d="M1"></path></svg><span>feed</span></a>'),
    );
  });

  it('merges text split by comments before resolving whitespace', () => {
    expect(normalize('<span><!--[-->a<!--]--><!--[--> <!--]--><!--[-->b<!--]--></span>')).toBe(
      normalize('<span>a b</span>'),
    );
  });

  it('compares attribute values verbatim apart from asset hashes', () => {
    expect(normalize('<img src="./assets/favicon-32-7967c1a90e5a.png" alt="">')).toBe(
      normalize('<img src="./assets/favicon-32.png" alt="">'),
    );
    expect(normalize('<span title="Published: a&#10;Updated: b"></span>')).toBe(
      normalize('<span title="Published: a\nUpdated: b"></span>'),
    );
    expect(normalize('<a class="x y"></a>')).not.toBe(normalize('<a class="y x"></a>'));
  });

  it('reads noscript content as markup whether or not it was escaped', () => {
    expect(normalize('<noscript>&lt;p class="x"&gt;Needs &lt;a href="./b/"&gt;JS&lt;/a&gt;.&lt;/p&gt;</noscript>')).toBe(
      normalize('<noscript><p class="x">Needs <a href="./b/">JS</a>.</p></noscript>'),
    );
  });

  it('prints one node per line', () => {
    expect(normalize('<div class="a"><span>x</span><p>y <b>z</b></p></div>')).toBe(
      ['<div class="a">', '  <span>x</span>', '  <p>', '    y ', '    <b>z</b>', '  </p>', '</div>'].join('\n'),
    );
  });
});
