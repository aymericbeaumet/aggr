// The render-blocking script in `<head>`, built as a classic IIFE by the classic-script target in build.ts:
// saved preferences onto `<html>` and dates in the preferred style before the first paint. It
// shares the app's rules and formatters and leaves nothing on `window`.
import { bootstrap } from './preferences/bootstrap';

bootstrap(document, (key) => localStorage.getItem(key));
