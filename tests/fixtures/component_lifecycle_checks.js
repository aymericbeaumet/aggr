const base = arguments[0];
const finish = arguments[arguments.length - 1];

(async () => {
  let checks = 0;
  const check = (value, message) => { checks++; if (!value) throw new Error(message); };
  const settle = () => new Promise(resolve => setTimeout(resolve, 25));
  async function until(predicate, message) {
    const started = performance.now();
    while (!predicate()) {
      if (performance.now() - started > 5000) throw new Error(message);
      await settle();
    }
    await settle();
  }
  const q = selector => document.querySelector(selector);
  await settle();
  const dialog = q('#shortcut-help');
  const identity = window.__componentLifecycleIdentity = {};
  async function navigate(route) {
    const url = new URL(route, base);
    await window.swup.navigate(url.href);
    await until(() => location.pathname === url.pathname && !window.swup.navigating, 'navigation completed: ' + route);
    check(window.__componentLifecycleIdentity === identity, 'navigation must retain the document');
    check(q('#shortcut-help') === dialog && document.querySelectorAll('#shortcut-help').length === 1, 'help is persistent and mounted once');
  }
  function change(control, value) {
    if (control.type === 'checkbox') control.checked = value;
    else control.value = value;
    control.dispatchEvent(new Event('change', { bubbles: true }));
  }
  check(!!q('[data-preferences-root]') && !!q('[data-shortcut-help-root]'), 'current component roots are present');
  const originalStorage = Storage.prototype.setItem;
  let writes = 0;
  Storage.prototype.setItem = function (key, value) {
    if (this === localStorage && key === 'aggr:theme') writes++;
    return originalStorage.call(this, key, value);
  };
  try {
    for (let cycle = 0; cycle < 3; cycle++) {
      await until(() => q('#theme-mode') && document.querySelectorAll('[data-preference]').length === 18, 'one complete preferences panel');
      const beforeWrites = writes;
      change(q('#theme-mode'), 'dark');
      await until(() => document.documentElement.dataset.theme === 'dark', 'theme applies');
      check(writes === beforeWrites + 1, 'one preference interaction produces one storage write');

      const peer = document.createElement('iframe');
      peer.hidden = true;
      peer.src = new URL('categories/', base).href;
      document.body.append(peer);
      await until(() => peer.contentWindow?.AGGRPreferences, 'same-origin peer tab loaded');
      peer.contentWindow.localStorage.setItem('aggr:theme', 'sepia');
      await until(() => q('#theme-mode').value === 'sepia' && document.documentElement.dataset.theme === 'sepia', 'cross-tab storage refreshes mounted controls');
      peer.remove();

      const transfer = new DataTransfer();
      transfer.items.add(new File([JSON.stringify({ version: 1, preferences: { theme: 'light', density: 'comfortable' } })], 'preferences.json', { type: 'application/json' }));
      q('#preferences-file').files = transfer.files;
      q('#preferences-file').dispatchEvent(new Event('change', { bubbles: true }));
      await until(() => !q('#preferences-import').hidden && q('#preferences-import-summary').children.length === 2, 'file import renders review');
      check(q('#theme-mode').value === 'sepia', 'review does not apply imported settings');
      q('[data-preferences-action="apply"]').click();
      await until(() => q('#theme-mode').value === 'light' && q('#density').value === 'comfortable' && q('#preferences-import').hidden, 'import applies to reactive controls');
      q('[data-preferences-action="reset"]').click();
      await until(() => q('#theme-mode').value === String(window.AGGRPreferences.schema.theme.initial), 'reset updates mounted controls');
      check(window.AGGRPreferences.values.density === window.AGGRPreferences.schema.density.initial, 'reset uses shared defaults');

      const trigger = q('#show-shortcuts');
      trigger.focus(); trigger.click();
      await until(() => dialog.open && document.activeElement === q('#shortcut-help-title'), 'help opens with heading focus');
      check(dialog.querySelectorAll('.shortcut-groups').length === 1, 'one set of help controls');
      for (const discussion of window.AGGR.discussions || []) {
        if (discussion.shortcut) check(dialog.textContent.includes(`Open ${discussion.name} discussion / search`), 'configured discussion shortcut remains available');
      }
      dialog.querySelector('.shortcut-close').click();
      await until(() => !dialog.open && document.activeElement === trigger, 'native close restores trigger focus');
      trigger.click();
      await until(() => dialog.open, 'help reopens');
      dialog.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await until(() => !dialog.open, 'backdrop closes help');

      const detachedPreference = q('#theme-mode');
      const detachedReset = q('[data-preferences-action="reset"]');
      await navigate('items/example/2026-09-01-story-36/');
      const savedTheme = window.AGGRPreferences.values.theme;
      const savedWrites = writes;
      change(detachedPreference, 'dark'); detachedReset.click();
      await settle();
      check(!detachedPreference.isConnected && window.AGGRPreferences.values.theme === savedTheme && writes === savedWrites, 'detached preference controls cannot mutate shared state');
      await until(() => q('.native-audio.is-enhanced [data-audio-toggle]'), 'podcast controls mount');
      const card = q('.native-audio');
      const audio = card.querySelector('audio');
      check(card.querySelectorAll('audio').length === 1 && card.querySelectorAll('[data-audio-toggle]').length === 1, 'one native media element and one set of controls');
      check(audio.controls && !!audio.querySelector('a[href]') && audio.preload === 'none', 'native fallback attributes and original link survive enhancement');
      const height = card.getBoundingClientRect().height;
      audio.preload = 'metadata'; audio.load();
      await until(() => audio.readyState >= 1, 'fixture audio metadata loads');
      check(Math.abs(card.getBoundingClientRect().height - height) <= 1, 'loading media metadata preserves reserved geometry');
      check(!card.querySelector('[data-audio-seek]').disabled, 'metadata enables seeking');
      check(!card.querySelector('[data-audio-native]') && !card.textContent.includes('Browser controls'), 'the player has no browser-controls toggle');
      const coverBounds = card.querySelector('.audio-cover').getBoundingClientRect();
      const artworkBounds = card.querySelector('.audio-artwork').getBoundingClientRect();
      check(getComputedStyle(card.querySelector('.audio-artwork')).objectFit === 'contain', 'podcast artwork remains entirely visible without cropping');
      const panelBounds = card.querySelector('.audio-panel').getBoundingClientRect();
      check(Math.abs(coverBounds.height - panelBounds.height) <= 1, 'the artwork area fills the entire control-panel height');
      check(Math.abs(panelBounds.width / coverBounds.width - 1.618) < .02, 'artwork and controls use golden-ratio columns');
      check(artworkBounds.width < coverBounds.width && artworkBounds.height < coverBounds.height, 'the artwork is inset on the player background, not a separate panel');
      check(artworkBounds.right <= panelBounds.left + 1, 'podcast controls stay to the right of the artwork');
      check(document.querySelector('.itemhead .reading-stats').textContent.trim() === '1 min listen', 'loaded audio duration replaces show-note read time');
      const mute = card.querySelector('[data-audio-mute]');
      mute.click(); await settle();
      check(audio.muted && mute.getAttribute('aria-pressed') === 'true', 'mute reflects actual media state');
      mute.click(); await settle();
      check(!audio.muted, 'mute restores playback sound');
      check(!card.querySelector('[data-audio-sleep]') && !card.textContent.toLowerCase().includes('sleep'), 'podcast controls have no sleep timer');
      Object.defineProperty(audio, 'duration', {configurable:true,value:600});
      Object.defineProperty(audio, 'currentTime', {configurable:true,writable:true,value:60});
      audio.playbackRate = 2;
      audio.dispatchEvent(new Event('durationchange'));
      audio.dispatchEvent(new Event('ratechange'));
      let plays = 0;
      audio.play = () => { plays++; audio.dispatchEvent(new Event('playing')); return Promise.resolve(); };
      const detachedToggle = card.querySelector('[data-audio-toggle]');
      detachedToggle.click();
      await settle();
      check(plays === 1, 'one click invokes one playback action');
      check(card.querySelector('[data-audio-ends-at]').textContent.includes('Ends at'), 'playing audio shows an end time at its selected speed');
      check(Math.abs(card.getBoundingClientRect().height - height) <= 1, 'controls and playback status keep player geometry stable');
      await navigate('');
      check(!detachedToggle.isConnected && audio.paused, 'leaving a page disposes playback');
      detachedToggle.click(); audio.dispatchEvent(new Event('playing'));
      await settle();
      check(plays === 1 && !card.classList.contains('is-enhanced') && !card.hasAttribute('data-audio-state'), 'detached audio controls and media events are inert');
      await navigate('preferences/');
    }
    finish({ checks, cycles: 3 });
  } finally {
    Storage.prototype.setItem = originalStorage;
  }
})().catch(error => finish({ error: error.stack || String(error), url: location.href }));
