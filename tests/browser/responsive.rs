//! Viewport widths: no horizontal overflow, one search field, and matching desktop density.

use anyhow::Result;
use fantoccini::Client;
use serde_json::json;

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, phone_session, report_failure,
    screenshot, wait_booted, wait_booted_with,
};
use crate::mobile::mobile_feed_layout;

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn responsive_widths_desktop_density_and_large_text() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(responsive_contracts(&client, &fixture)).await;
    report_failure(&client, "responsive", &result).await;
    finish(client, result).await
}

async fn responsive_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute("localStorage.setItem('aggr:theme','light')", vec![])
        .await?;
    client.refresh().await?;
    wait_booted_with(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    // The compact phone header is the reference the desktop header must match.
    let layout = mobile_feed_layout(client).await?;
    // These contracts used to run right after the preferences contract, which left hidden
    // thumbnails, ISO dates, reduced motion and a wide reading width behind; the compact desktop
    // row budget below was measured in that state (a visible 48px thumbnail makes a row taller).
    client
        .execute(
            "localStorage.setItem('aggr:thumbnails','hide');localStorage.setItem('aggr:date-format','iso');localStorage.setItem('aggr:motion','off');localStorage.setItem('aggr:reading-width','wide')",
            vec![],
        )
        .await?;
    client
        .execute(
            "localStorage.setItem('aggr:theme','light');localStorage.setItem('aggr:density','compact');localStorage.setItem('aggr:text-size','default')",
            vec![],
        )
        .await?;
    for width in [320, 375, 430, 768, 1280] {
        emulate(
            client,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<640}),
        )
        .await?;
        client.goto(&fixture.base).await?;
        let geometry = client
            .execute(
                r#"
          return {overflow:document.documentElement.scrollWidth > document.documentElement.clientWidth+1,
            searches:document.querySelectorAll('#q').length};
        "#,
                vec![],
            )
            .await?;
        assert_eq!(
            geometry,
            json!({"overflow":false,"searches":1}),
            "width={width}"
        );
    }
    emulate(
        client,
        "Emulation.setTouchEmulationEnabled",
        json!({"enabled":false}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    assert_eq!(
        client
            .execute("return window.swup.options.native", vec![])
            .await?,
        false,
        "desktop navigation must also skip full-page transition snapshots"
    );
    let desktop_density = client
        .execute(
            r#"
      const rows = Array.from(document.querySelectorAll('.row')).slice(1);
      const row = rows[0];
      const title = row.querySelector('.title').getBoundingClientRect();
      const metadata = row.querySelector('.meta').getBoundingClientRect();
      const meta = row.querySelector('.meta > *').getBoundingClientRect();
      const rank = row.querySelector('.rank');
      const rankText = document.createRange();
      rankText.selectNodeContents(rank);
      const top = document.querySelector('.top');
      const topStyle = getComputedStyle(top);
      const topNavStyle = getComputedStyle(document.querySelector('.nav'));
      const brandLinkStyle = getComputedStyle(document.querySelector('.brand'));
      const configStyle = getComputedStyle(document.querySelector('.config-link'));
      const separatorLeft = row.getBoundingClientRect().left + parseFloat(getComputedStyle(row, '::after').left);
      return {height:row.getBoundingClientRect().height,
        metadataBelowTitle:metadata.top >= title.bottom - 1,
        metaLeft:meta.left,titleLeft:title.left,separatorLeft,
        markerRight:row.getBoundingClientRect().left + parseFloat(getComputedStyle(row, '::before').left) + parseFloat(getComputedStyle(row, '::before').width),
        rankTextLeft:rankText.getBoundingClientRect().left,
        brandLeft:document.querySelector('.brand-icon').getBoundingClientRect().left,
        header:{height:top.getBoundingClientRect().height,position:topStyle.position,
          background:topStyle.backgroundColor,navMinHeight:topNavStyle.minHeight,
          navPaddingLeft:topNavStyle.paddingLeft,navPaddingRight:topNavStyle.paddingRight,
          brandMarginLeft:brandLinkStyle.marginLeft,brandMarginRight:brandLinkStyle.marginRight,
          configFontSize:configStyle.fontSize,configPaddingLeft:configStyle.paddingLeft,
          configPaddingRight:configStyle.paddingRight},
        capacity:Math.floor((innerHeight-document.querySelector('.top').getBoundingClientRect().height)/row.getBoundingClientRect().height)};
    "#,
            vec![],
        )
        .await?;
    assert!(
        desktop_density["height"].as_f64().unwrap_or(f64::MAX) <= 48.0
            && desktop_density["capacity"].as_u64().unwrap_or_default() >= 16,
        "compact desktop rows should fit roughly 18 entries without shrinking titles: {desktop_density}"
    );
    assert_eq!(desktop_density["metadataBelowTitle"], true);
    let mut mobile_header = layout["header"].clone();
    let mut desktop_header = desktop_density["header"].clone();
    let mobile_height = mobile_header
        .as_object_mut()
        .unwrap()
        .remove("height")
        .unwrap();
    let desktop_height = desktop_header
        .as_object_mut()
        .unwrap()
        .remove("height")
        .unwrap();
    assert!(
        (mobile_height.as_f64().unwrap() - desktop_height.as_f64().unwrap()).abs() <= 1.0,
        "mobile header stays a single compact row with navigation in the bottom bar"
    );
    assert_eq!(
        desktop_header, mobile_header,
        "responsive headers should share colors, spacing, and touch targets"
    );
    assert!(
        (desktop_density["metaLeft"].as_f64().unwrap_or_default()
            - desktop_density["titleLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0,
        "desktop metadata should share the title text axis: {desktop_density}"
    );
    assert!(
        (desktop_density["rankTextLeft"].as_f64().unwrap_or_default()
            - desktop_density["brandLeft"].as_f64().unwrap_or_default()
            - 8.0)
            .abs()
            <= 1.0,
        "desktop rows should share the extra 8px inset used on mobile: {desktop_density}"
    );
    assert!(
        (desktop_density["markerRight"].as_f64().unwrap_or_default()
            - desktop_density["separatorLeft"]
                .as_f64()
                .unwrap_or_default())
        .abs()
            <= 1.0,
        "desktop separators should meet the right edge of the selection bar: {desktop_density}"
    );
    screenshot(client, "desktop-feed").await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client
        .execute_async("const done=arguments[arguments.length-1];document.documentElement.style.fontSize='200%';requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return document.documentElement.scrollWidth <= document.documentElement.clientWidth+1",
                vec![]
            )
            .await?,
        true,
        "overflowing elements at 200% text: {}",
        client.execute("return {overflow:[...document.querySelectorAll('body *')].map(e=>({tag:e.tagName,cls:e.className,right:e.getBoundingClientRect().right,width:e.getBoundingClientRect().width})).filter(e=>e.right>document.documentElement.clientWidth+1).slice(0,12),layout:['html','body','.main','.rows','.row','.feed-toolbar'].map(s=>{const e=document.querySelector(s),c=getComputedStyle(e),r=e.getBoundingClientRect();return {s,left:r.left,width:r.width,pad:c.padding,margin:c.margin,font:c.fontSize}})}",vec![]).await?
    );
    client
        .execute(
            "document.documentElement.style.fontSize='';localStorage.setItem('aggr:theme','dark')",
            vec![],
        )
        .await?;
    Ok(())
}
