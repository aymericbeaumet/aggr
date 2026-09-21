import type { ReaderWindow } from './contracts';
import { applyInitialPreferences, createPreferences } from './preferences/bootstrap';
import { createDateText, formatBeforePaint } from './prepaint-dates';

const defaults = JSON.parse(document.getElementById('aggr-preferences')?.textContent || '{}');
const preferences = createPreferences(defaults, key => localStorage.getItem(key));
(window as unknown as ReaderWindow).AGGRPreferences = preferences;
applyInitialPreferences(preferences, document.documentElement.dataset);

const text = createDateText();
(window as Window & { AGGRDates?: { text: typeof text } }).AGGRDates = { text };
formatBeforePaint(document, timestamp => text(timestamp, preferences.values['date-format']));
