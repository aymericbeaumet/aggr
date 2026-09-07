const template = arguments[0];
const finish = arguments[arguments.length - 1];

try {
  const script = template.match(/<script id="aggr-preferences">([\s\S]*?)<\/script>/)[1];
  const storage = new Map([
    ["aggr:theme", "dark"],
    ["aggr:date-format", "iso"],
    ["aggr:single-key-shortcuts", "false"],
    ["aggr:density", "invalid"],
    ["aggr:reading-history", "private"]
  ]);
  let writes = 0;
  const document = { documentElement: { dataset: {} } };
  const window = {};
  const localStorage = { getItem: key => storage.get(key), setItem: () => { writes += 1; } };
  new Function("window", "document", "localStorage", script)(window, document, localStorage);
  const preferences = window.AGGRPreferences;
  function check(value, message) { if (!value) throw new Error(message); }
  function rejects(value) {
    let rejected = false;
    try { preferences.validate(value); } catch (error) { rejected = true; }
    check(rejected, "Accepted invalid preferences: " + JSON.stringify(value));
  }
  check(document.documentElement.dataset.theme === "dark", "Stored theme must apply before paint");
  check(document.documentElement.dataset.density === "compact", "Invalid stored values must fall back to defaults");
  check(preferences.values["date-format"] === "iso" && preferences.values["single-key-shortcuts"] === false, "Existing keys must remain compatible");
  check(Object.keys(preferences.values).length === 9 && preferences.values["feed-page-size"] === "50" && !JSON.stringify(preferences.values).includes("private"), "Only appearance/interaction preferences with a 50-item feed default belong in exported state");
  const full = { version: 1, preferences: {} };
  Object.keys(preferences.schema).forEach(key => {
    full.preferences[key] = preferences.schema[key].values.slice(-1)[0];
    preferences.schema[key].values.forEach(value => {
      check(preferences.validate({ version: 1, preferences: { [key]: value } })[key] === value, "Every option must round-trip with its original type");
    });
  });
  check(JSON.stringify(preferences.validate(JSON.parse(JSON.stringify(full)))) === JSON.stringify(full.preferences), "All options must share one serialization contract");
  const legacy = preferences.validate({ "aggr:theme": "light", "aggr:date-format": "local-time", "aggr:single-key-shortcuts": "false" });
  check(legacy.theme === "light" && legacy["single-key-shortcuts"] === false, "Old shared links must import typed preferences");
  [null, [], {}, { version: 2, preferences: { theme: "dark" } }, { version: 1, preferences: null },
    { version: 1, preferences: { theme: "purple" } }, { version: 1, preferences: { "single-key-shortcuts": "false" } },
    { version: 1, preferences: { theme: "dark", history: "private" } },
    { version: 1, preferences: { theme: "dark" }, extra: true },
    { "aggr:theme": "dark", "aggr:single-key-shortcuts": false },
    JSON.parse('{"version":1,"preferences":{"__proto__":{"polluted":true}}}')
  ].forEach(rejects);
  check(writes === 0 && storage.get("aggr:theme") === "dark", "Loading or validating imports must not mutate local preferences");
  const unavailable = {};
  new Function("window", "document", "localStorage", script)(unavailable, { documentElement: { dataset: {} } }, {
    getItem: () => { throw new Error("Storage blocked"); }
  });
  check(unavailable.AGGRPreferences.values.theme === "auto", "Private browsing must still produce valid defaults");
  finish({ checks: 10 });
} catch (error) {
  finish({ error: error.stack || String(error) });
}
