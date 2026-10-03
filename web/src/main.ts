import { boot } from './app/boot';

// A prerendered page runs before anyone has seen it: writing session state or history there
// would record visits that never happened.
if (document.prerendering) document.addEventListener('prerenderingchange', boot, { once: true });
else boot();
