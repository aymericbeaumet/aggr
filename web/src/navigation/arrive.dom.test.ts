import { afterEach, describe, expect, it } from 'vitest';
import { arrive } from './arrive';

const nextFrame = () => new Promise((resolve) => requestAnimationFrame(resolve));

function page(): { main: HTMLElement; field: HTMLInputElement } {
  const main = document.createElement('main');
  main.id = 'content';
  main.tabIndex = -1;
  const field = document.createElement('input');
  field.id = 'q';
  field.type = 'search';
  main.append(field);
  document.body.append(main);
  return { main, field };
}

afterEach(() => {
  document.body.replaceChildren();
});

describe('arrive', () => {
  it('leaves a control inside the page focused', async () => {
    const { main, field } = page();
    arrive(undefined, '');
    field.focus();
    await nextFrame();
    expect(document.activeElement).toBe(field);
    expect(main.hasAttribute('data-navigation-focus')).toBe(false);
  });

  it('focuses the page when nothing inside it has the keyboard', async () => {
    const { main } = page();
    arrive(undefined, '');
    await nextFrame();
    expect(document.activeElement).toBe(main);
    expect(main.hasAttribute('data-navigation-focus')).toBe(true);
  });

  it('focuses the page when the keyboard is still on the header', async () => {
    const { main } = page();
    const brand = document.createElement('a');
    brand.href = '/';
    brand.className = 'brand';
    document.body.append(brand);
    brand.focus();
    arrive(undefined, '');
    await nextFrame();
    expect(document.activeElement).toBe(main);
  });
});
