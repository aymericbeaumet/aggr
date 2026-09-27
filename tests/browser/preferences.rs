//! The preferences page: controls, applied values, import review and link transfer.

use anyhow::{Context as _, Result};
use fantoccini::{Client, Locator};
use serde_json::json;

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, key, phone_session, report_failure,
    screenshot, wait_booted, wait_booted_with, wait_for,
};
use crate::mobile::mobile_feed_layout;

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn preference_controls_transfer_and_import() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(preference_contracts(&client, &fixture)).await;
    report_failure(&client, "preferences", &result).await;
    finish(client, result).await
}

async fn preference_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    // The bootstrap's validation rules are covered by web/src/preferences/bootstrap.test.ts; the
    // browser confirms the shared script runs before paint: the stored theme is on <html> while the
    // head is still parsing and the app has not loaded, and an invalid stored value has already
    // fallen back to its default.
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute(
            "localStorage.setItem('aggr:theme','dark');localStorage.setItem('aggr:density','invalid');",
            vec![],
        )
        .await?;
    emulate(
        client,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": r#"
          new MutationObserver((records, observer) => {
            if (!records.some(record => record.attributeName === 'data-theme')) return;
            observer.disconnect();
            window.__aggrBootstrap = {
              theme: document.documentElement.dataset.theme,
              density: document.documentElement.dataset.density,
              headParsing: document.body === null && document.readyState === 'loading',
              appLoaded: document.documentElement.dataset.aggrReady === 'true'
            };
          }).observe(document, {attributes: true, subtree: true});
        "#}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    assert_eq!(
        client
            .execute("return window.__aggrBootstrap || null", vec![])
            .await?,
        json!({"theme":"dark","density":"compact","headParsing":true,"appLoaded":false}),
        "the shared preferences script must apply stored values before paint and fall back for invalid ones"
    );
    client
        .execute(
            "localStorage.removeItem('aggr:theme');localStorage.removeItem('aggr:density');",
            vec![],
        )
        .await?;
    client.goto(&fixture.base).await?;
    wait_booted_with(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    let compact_row_height = mobile_feed_layout(client).await?["rowHeight"]
        .as_f64()
        .context("compact row height")?;
    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-preference]').length === 17",
    )
    .await?;
    let preference_controls = client
        .execute(
            r#"
      return {
        ids:Array.from(document.querySelectorAll('[data-preference]')).map(control => control.id),
        groups:Array.from(document.querySelectorAll('.preferences-group:not(.preferences-import) > h2')).map(heading => heading.textContent),
        install:!!document.querySelector('#install-app'),
        config:!!document.querySelector('.preferences-config'),
        shortcutLink:document.querySelector('#show-shortcuts')?.tagName,
        defaults:Object.fromEntries(Array.from(document.querySelectorAll('[data-preference]')).map(control => [control.dataset.preference, control.type === 'checkbox' ? control.checked : control.value]))
      };
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        preference_controls["ids"],
        json!([
            "theme",
            "motion",
            "font-family",
            "text-size",
            "reading-width",
            "line-spacing",
            "paragraph-spacing",
            "paragraph-indent",
            "text-align",
            "letter-spacing",
            "word-spacing",
            "density",
            "thumbnails",
            "feed-page-size",
            "date-format",
            "scroll-amount",
            "single-key-shortcuts"
        ])
    );
    assert_eq!(
        preference_controls["groups"],
        json!(["Appearance", "Reading", "Feed", "Keyboard"])
    );
    assert_eq!(
        preference_controls["defaults"]["single-key-shortcuts"],
        true
    );
    assert_eq!(preference_controls["defaults"]["feed-page-size"], "50");
    assert_eq!(preference_controls["install"], false);
    assert_eq!(preference_controls["config"], false);
    // Opening a dialog is a button's job, not a link's: there is no page to go to.
    assert_eq!(preference_controls["shortcutLink"], "BUTTON");
    client
        .execute(
            "document.querySelector('#show-shortcuts').scrollIntoView({block:'center'})",
            vec![],
        )
        .await?;
    client
        .find(Locator::Css("#show-shortcuts"))
        .await?
        .click()
        .await?;
    wait_for(client, "document.querySelector('#shortcut-help').open").await?;
    assert_eq!(
        client.current_url().await?.path(),
        "/reader/preferences/",
        "the shortcut helper link must stay on Preferences"
    );
    assert_eq!(
        client
            .execute(
                "return {checked:document.querySelector('#single-key-shortcuts').checked, decoration:getComputedStyle(document.querySelector('#show-shortcuts')).textDecorationLine}",
                vec![]
            )
            .await?,
        json!({"checked":true,"decoration":"underline"})
    );
    // Every alternative for one action reads as one mapping: the key column is sized to hold the
    // widest of them, so none of them is folded onto a second line. A mapping that differs by
    // platform names only the modifier this one has.
    let mappings = client
        .execute(
            r#"
      const keys = [...document.querySelectorAll('#shortcut-help .shortcut-list dt')];
      // Every alternative of one mapping starts on the same line as the first.
      const wrapped = keys.filter(key => {
        // Platform-specific mappings ship both modifiers and hide one, and a hidden key has no
        // box to compare against.
        const parts = [...key.querySelectorAll('kbd, small')].filter(part => part.getClientRects().length > 0);
        if (parts.length < 2) return false;
        const boxes = parts.map(part => part.getBoundingClientRect());
        return boxes.some(box => box.top >= boxes[0].bottom - 1);
      }).map(key => key.textContent.trim());
      const search = keys.find(key => key.textContent.includes('/'));
      const shown = [...document.querySelectorAll('#shortcut-help [data-platform-key]')]
        .filter(pair => pair.getClientRects().length > 0)
        .map(pair => pair.dataset.platformKey);
      return {wrapped, search: !!search, shown, platform: document.documentElement.dataset.platform};
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        mappings["wrapped"],
        json!([]),
        "shortcut alternatives share one line: {mappings}"
    );
    assert_eq!(mappings["search"], true, "the help lists / for search");
    assert_eq!(
        mappings["shown"].as_array().map(Vec::len),
        Some(1),
        "exactly one modifier is offered for a platform-specific mapping: {mappings}"
    );
    assert_eq!(
        mappings["shown"][0], mappings["platform"],
        "the modifier shown is the one this platform presses: {mappings}"
    );
    key(client, "Escape").await?;
    let applied_preferences = client
        .execute(
            r#"
      const values = {
        theme:'dark', motion:'off', 'text-size':'large', 'reading-width':'wide',
        density:'comfortable', thumbnails:'hide', 'date-format':'iso'
      };
      Object.keys(values).forEach(function (id) {
        const control = document.getElementById(id);
        control.value = values[id];
        control.dispatchEvent(new Event('change', {bubbles:true}));
      });
      return {
        theme:document.documentElement.dataset.theme,
        motion:document.documentElement.dataset.motion,
        textSize:document.documentElement.dataset.textSize,
        readingWidth:document.documentElement.dataset.readingWidth,
        density:document.documentElement.dataset.density,
        thumbnails:document.documentElement.dataset.thumbnails,
        date:localStorage.getItem('aggr:date-format')
      };
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        applied_preferences,
        json!({"theme":"dark","motion":"off","textSize":"large","readingWidth":"wide",
          "density":"comfortable","thumbnails":"hide","date":"iso"})
    );
    screenshot(client, "mobile-preferences").await?;

    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && document.documentElement.dataset.density === 'comfortable'",
    )
    .await?;
    let comfortable_row = client
        .execute(
            r#"
      const row = document.querySelectorAll('.row')[1];
      const title = row.querySelector('.title');
      row.classList.remove('is-selected');
      const plain = getComputedStyle(row).backgroundColor;
      const plainInset = title.getBoundingClientRect().left-row.getBoundingClientRect().left;
      row.classList.add('is-selected');
      return {
        height:row.getBoundingClientRect().height,
        inset:title.getBoundingClientRect().left-row.getBoundingClientRect().left,
        plainInset,
        contentInset:parseFloat(getComputedStyle(row).paddingLeft),
        padding:parseFloat(getComputedStyle(row).paddingTop),
        plain:plain,
        selected:getComputedStyle(row).backgroundColor
      };
    "#,
            vec![],
        )
        .await?;
    assert!(
        comfortable_row["height"].as_f64().unwrap_or_default() >= compact_row_height + 15.0,
        "comfortable density should add deliberate space: compact={compact_row_height}, comfortable={comfortable_row}"
    );
    assert!(
        comfortable_row["padding"].as_f64().unwrap_or_default() >= 9.5
            && (comfortable_row["inset"].as_f64().unwrap_or_default()
                - comfortable_row["contentInset"].as_f64().unwrap_or_default())
            .abs()
                <= 1.0
            && comfortable_row["inset"] == comfortable_row["plainInset"],
        "comfortable selected rows need padding without losing alignment: {comfortable_row}"
    );
    assert_eq!(
        comfortable_row["plain"], comfortable_row["selected"],
        "selection must leave the row background unchanged at every density"
    );

    client
        .goto(&format!("{}preferences/", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-preference]').length === 17",
    )
    .await?;
    client
        .execute(
            "const control=document.querySelector('#single-key-shortcuts');control.checked=false;control.dispatchEvent(new Event('change',{bubbles:true}));",
            vec![],
        )
        .await?;

    client
        .execute(
            "Object.defineProperty(Navigator.prototype, 'clipboard', {configurable:true, get:function () { return undefined; }});",
            vec![],
        )
        .await?;
    client
        .execute("document.querySelector('#copy-state').click();", vec![])
        .await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-link').hidden && document.querySelector('#preferences-link').value.includes('#aggr-state=')",
    )
    .await?;
    let copied_link = client
        .find(Locator::Css("#preferences-link"))
        .await?
        .prop("value")
        .await?
        .context("copied preferences link")?;
    let copied_link_contract = client
        .execute(
            r#"
      const url = new URL(document.querySelector('#preferences-link').value);
      return {path:url.pathname, fragment:url.hash.startsWith('#aggr-state='), query:url.searchParams.has('aggr-state')};
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        copied_link_contract,
        json!({"path":"/reader/preferences/","fragment":true,"query":false})
    );

    let ignored_transfer = client.execute(r#"
      const url=new URL('preferences/',new URL(window.AGGR.base,location.href));
      url.searchParams.set('aggr-state',btoa(JSON.stringify({version:1,preferences:{theme:'light'}})));
      return url.href;
    "#,vec![]).await?.as_str().context("query-string transfer URL")?.to_owned();
    client.goto(&ignored_transfer).await?;
    wait_for(
        client,
        "document.querySelector('#theme') && document.querySelector('#preferences-import')",
    )
    .await?;
    anyhow::ensure!(client.execute("return document.documentElement.dataset.theme==='dark' && document.querySelector('#theme').value==='dark' && document.querySelector('#preferences-import').hidden",vec![]).await?==true,"query-string preference transfers are ignored without opening a review or changing settings");

    let valid_preferences = fixture.directory.path().join("valid-preferences.json");
    let invalid_preferences = fixture.directory.path().join("invalid-preferences.json");
    std::fs::write(
        &valid_preferences,
        r#"{"version":1,"preferences":{"theme":"light","density":"compact"}}"#,
    )?;
    std::fs::write(
        &invalid_preferences,
        r#"{"version":1,"preferences":{"theme":"ultraviolet"}}"#,
    )?;
    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            valid_preferences
                .to_str()
                .context("preferences file path")?,
        )
        .await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-import').hidden && document.querySelectorAll('#preferences-import-summary li').length === 2",
    )
    .await?;
    let before_apply = client
        .execute(
            "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,storedTheme:localStorage.getItem('aggr:theme'),storedDensity:localStorage.getItem('aggr:density')};",
            vec![],
        )
        .await?;
    assert_eq!(
        before_apply,
        json!({"theme":"dark","density":"comfortable","storedTheme":"dark","storedDensity":"comfortable"}),
        "import review must not mutate preferences"
    );
    client
        .execute(
            "document.querySelector(\"[data-preferences-action='apply']\").click();",
            vec![],
        )
        .await?;
    wait_for(
        client,
        "document.documentElement.dataset.theme === 'light' && document.documentElement.dataset.density === 'compact' && document.querySelector('#preferences-import').hidden",
    )
    .await?;

    client.goto(&fixture.base).await?;
    client.goto(&copied_link).await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-import').hidden && document.querySelectorAll('#preferences-import-summary li').length === 17",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,fragment:location.hash};",
                vec![]
            )
            .await?,
        json!({"theme":"light","density":"compact","fragment":""}),
        "fragment transfer must be reviewed and removed from browser history before applying"
    );
    client
        .execute(
            "document.querySelector(\"[data-preferences-action='apply']\").click();",
            vec![],
        )
        .await?;
    wait_for(
        client,
        "document.documentElement.dataset.theme === 'dark' && document.documentElement.dataset.density === 'comfortable'",
    )
    .await?;

    client.goto(&fixture.base).await?;
    client
        .goto(&format!(
            "{}preferences/#aggr-state=not_valid!",
            fixture.base
        ))
        .await?;
    wait_for(
        client,
        "document.querySelector('#preferences-status').textContent.includes('invalid or unsupported')",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,panel:document.querySelector('#preferences-import').hidden};",
                vec![]
            )
            .await?,
        json!({"theme":"dark","density":"comfortable","panel":true}),
        "invalid fragments must leave the stored state untouched"
    );
    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            invalid_preferences
                .to_str()
                .context("invalid preferences file path")?,
        )
        .await?;
    wait_for(
        client,
        "document.querySelector('#preferences-status').textContent.includes('file is invalid or unsupported')",
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return {theme:document.documentElement.dataset.theme,density:document.documentElement.dataset.density,panel:document.querySelector('#preferences-import').hidden};",
                vec![]
            )
            .await?,
        json!({"theme":"dark","density":"comfortable","panel":true}),
        "invalid files must leave the stored state untouched"
    );

    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            valid_preferences
                .to_str()
                .context("preferences file path")?,
        )
        .await?;
    wait_for(
        client,
        "!document.querySelector('#preferences-import').hidden",
    )
    .await?;
    client.execute("document.querySelector('[data-preferences-action=cancel]').scrollIntoView({block:'nearest',behavior:'instant'})", vec![]).await?;
    assert_eq!(client.execute("return document.querySelector('[data-preferences-action=cancel]').getBoundingClientRect().bottom < document.querySelector('.mobile-tabs').getBoundingClientRect().top", vec![]).await?, true, "native scrolling must keep preference controls above the fixed tab bar");
    client
        .find(Locator::Css("[data-preferences-action='cancel']"))
        .await?
        .click()
        .await?;
    wait_for(
        client,
        "document.querySelector('#preferences-status').textContent.includes('Import cancelled')",
    )
    .await?;
    std::fs::write(
        &invalid_preferences,
        r#"{"aggr:theme":"light","aggr:density":"compact"}"#,
    )?;
    client
        .find(Locator::Css("#preferences-file"))
        .await?
        .send_keys(
            invalid_preferences
                .to_str()
                .context("obsolete preference file path")?,
        )
        .await?;
    wait_for(client,"document.querySelector('#preferences-status').textContent.includes('file is invalid or unsupported')").await?;
    anyhow::ensure!(client.execute("return document.documentElement.dataset.theme==='dark' && document.documentElement.dataset.density==='comfortable' && document.querySelector('#preferences-import').hidden",vec![]).await?==true,"unversioned storage-key exports are rejected without changing preferences");
    Ok(())
}
