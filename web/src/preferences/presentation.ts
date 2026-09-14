import type { PreferenceValue } from "../contracts";

interface PreferenceField { label: string; options: Array<{ value: string; label: string }> }
function field(label: string, options: Record<string, string> = {}): PreferenceField {
  return { label, options: Object.entries(options).map(([value, label]) => ({ value, label })) };
}

export const fields: Record<string, PreferenceField> = {
  "theme": field("Theme", {"auto": "System", "light": "Light", "dark": "Dark", "sepia": "Sepia"}),
  "motion": field("Animations", {"auto": "Follow system", "off": "Off"}),
  "font-family": field("Typeface", {"sans": "Sans serif", "serif": "Serif", "mono": "Monospace"}),
  "text-size": field("Article text", {"default": "Default", "large": "Large", "largest": "Largest"}),
  "reading-width": field("Line width", {"narrow": "Narrow", "standard": "Standard", "wide": "Wide"}),
  "line-spacing": field("Line spacing", {"compact": "Compact", "standard": "Standard", "relaxed": "Relaxed"}),
  "paragraph-spacing": field("Paragraph spacing", {"compact": "Compact", "standard": "Standard", "relaxed": "Relaxed"}),
  "paragraph-indent": field("Indent paragraphs"),
  "text-align": field("Text alignment", {"left": "Start", "justify": "Justified"}),
  "letter-spacing": field("Letter spacing", {"normal": "Normal", "wide": "Wide"}),
  "word-spacing": field("Word spacing", {"normal": "Normal", "wide": "Wide"}),
  "density": field("Density", {"compact": "Compact", "comfortable": "Comfortable"}),
  "thumbnails": field("Preview images", {"show": "Show when available", "hide": "Hide"}),
  "feed-page-size": field("Maximum per page", {"10": "10", "25": "25", "50": "50"}),
  "date-format": field("Dates", {"relative": "Relative", "iso": "YYYY-MM-DD", "local": "Local date", "local-time": "Local date & time"}),
  "scroll-amount": field("d/u scroll distance (lines)"),
  "single-key-shortcuts": field("Enable keyboard shortcuts"),
  "offline-items": field("Recent articles to keep"),
};

export function preferenceDescription(key: string, value: PreferenceValue): string {
  const field = fields[key];
  const display = typeof value === "boolean" ? value ? "On" : "Off" : field.options.find(option => option.value === String(value))?.label || value;
  return `${field.label}: ${display}`;
}
