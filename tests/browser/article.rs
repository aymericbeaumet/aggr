//! Reading an article: layout, folding header, reading progress and keyboard navigation.

use anyhow::{Context as _, Result};
use fantoccini::{Client, Locator};
use serde_json::{Value, json};

use crate::harness::{
    Fixture, browser_client, catch_panics, emulate, finish, git, key, phone_session,
    report_failure, screenshot, wait_booted, wait_booted_with, wait_for,
};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn feed_cursor_new_item_fade_and_article_reading_layout() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(article_reading_contracts(&client, &fixture)).await;
    report_failure(&client, "article-reading", &result).await;
    finish(client, result).await
}

async fn article_reading_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute("localStorage.setItem('aggr:theme','light')", vec![])
        .await?;
    client.goto(&fixture.base).await?;
    key(client, "g").await?;
    key(client, "l").await?;
    wait_for(client, "location.pathname === '/reader/browse/'").await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3 && document.documentElement.classList.contains('swup-enabled')",
    )
    .await?;
    client.execute("sessionStorage.setItem('aggr:last-seen-entry:' + encodeURIComponent('/reader/'), document.querySelectorAll('[data-row-open]')[1].href)", vec![]).await?;
    client.refresh().await?;
    wait_for(client, "!!document.querySelector('.row') && document.documentElement.classList.contains('swup-enabled')").await?;
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.row.is-selected').length",
                vec![]
            )
            .await?,
        1,
        "an ordinary reload selects the first visible row"
    );
    assert_eq!(
        client
            .execute(
                "return document.querySelectorAll('.new-marker').length",
                vec![]
            )
            .await?,
        0
    );
    let new_item_fade = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      const row = document.querySelector('.row');
      const cell = row.querySelector('.cell');
      const background = getComputedStyle(cell, '::before');
      const separator = getComputedStyle(row, '::after');
      const aligned = background.left === separator.left && background.right === separator.right;
      const color = () => getComputedStyle(cell, '::before').backgroundColor;
      const highlighted = color();
      const duration = background.transitionDuration;
      const badge = document.querySelector('link[rel~=icon]').href.startsWith('data:');
      requestAnimationFrame(() => {
        const initial = color();
        setTimeout(() => {
          const middle = color();
          setTimeout(() => {
            const final = color();
            done({highlighted, initial, middle, final, aligned, duration, badge});
          }, 3500);
        }, 2000);
      });
    "#,
            vec![],
        )
        .await?;
    assert_eq!(new_item_fade["duration"], "5s");
    assert_eq!(
        new_item_fade["badge"], false,
        "opening the page acknowledges the favicon badge"
    );
    assert_eq!(
        new_item_fade["aligned"], true,
        "the new-item highlight must share both horizontal edges with the separator"
    );
    assert_ne!(
        new_item_fade["initial"], new_item_fade["final"],
        "acknowledged entries must retain their highlight at the start of the fade"
    );
    assert_ne!(
        new_item_fade["middle"], new_item_fade["initial"],
        "the highlight should fade progressively"
    );
    assert_ne!(
        new_item_fade["middle"], new_item_fade["final"],
        "the highlight should remain partially visible midway through the fade"
    );
    key(client, "k").await?;
    let first = client
        .execute("return document.activeElement.href", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return getComputedStyle(document.activeElement).textDecorationLine",
                vec![]
            )
            .await?,
        "none",
        "the selected row marker is sufficient without underlining its title"
    );
    key(client, "j").await?;
    let second = client
        .execute("return document.activeElement.href", vec![])
        .await?;
    assert_ne!(first, second);
    key(client, "k").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );
    client
        .execute("document.querySelector('.row .category a').focus()", vec![])
        .await?;
    key(client, "j").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        second,
        "j must keep moving the feed cursor from a focused row metadata link"
    );
    key(client, "k").await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":260,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    let scrolled = client
        .execute(
            r#"
      window.scrollTo(0, document.documentElement.scrollHeight);
      return {y:window.scrollY, max:document.documentElement.scrollHeight-document.documentElement.clientHeight};
    "#,
            vec![],
        )
        .await?;
    assert!(
        scrolled["y"].as_f64().unwrap_or_default() > 100.0,
        "fixture must exercise a genuinely scrolled feed: {scrolled}"
    );
    client
        .execute(
            r#"
      window.__aggrTransitionCalls = 0;
      if (document.startViewTransition) {
        const start = document.startViewTransition.bind(document);
        document.startViewTransition = function (update) {
          window.__aggrTransitionCalls++;
          return start(update);
        };
      }
    "#,
            vec![],
        )
        .await?;
    key(client, "o").await?;
    wait_for(client, "!!document.querySelector('.body pre')").await?;
    assert_eq!(
        client
            .execute("return window.__aggrTransitionCalls", vec![])
            .await?,
        0,
        "navigation should replace content without waiting for a page transition"
    );
    let scroll_samples = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      const samples = [];
      function sample() {
        samples.push(window.scrollY);
        if (samples.length === 10) done(samples);
        else requestAnimationFrame(sample);
      }
      requestAnimationFrame(sample);
    "#,
            vec![],
        )
        .await?;
    assert!(
        scroll_samples
            .as_array()
            .context("article scroll samples")?
            .iter()
            .all(|value| value.as_f64().unwrap_or_default().abs() <= 0.5),
        "article scroll position must stay at the top throughout the transition: {scroll_samples}"
    );
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1014,"height":700,"deviceScaleFactor":1,"mobile":false}),
    )
    .await?;
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let pinned_header = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      window.scrollTo(0, 0);
      requestAnimationFrame(function () {
        const head = document.querySelector('.itemhead');
        const title = head.querySelector('h1');
        const main = document.querySelector('.main');
        const top = document.querySelector('.top');
        const headStyle = getComputedStyle(head);
        const titleStyle = getComputedStyle(title);
        const initial = {headTop:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,titleLeft:title.getBoundingClientRect().left,
          mainTop:main.getBoundingClientRect().top,mainPadding:getComputedStyle(main).paddingTop,
          mainPaddingLeft:parseFloat(getComputedStyle(main).paddingLeft),
          headPaddingTop:parseFloat(headStyle.paddingTop),headPaddingBottom:parseFloat(headStyle.paddingBottom),
          titleMarginBottom:parseFloat(titleStyle.marginBottom),
          topHeight:top.getBoundingClientRect().height,topOffset:getComputedStyle(document.documentElement).getPropertyValue('--top-nav-offset')};
        window.scrollTo(0, 520);
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            const stuck = {headTop:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,titleLeft:title.getBoundingClientRect().left,scrollY:window.scrollY};
            window.scrollTo(0, 0);
            requestAnimationFrame(function () { done({initial,stuck}); });
          });
        });
      });
    "#,
            vec![],
        )
        .await?;
    assert!(
        pinned_header["stuck"]["scrollY"]
            .as_f64()
            .unwrap_or_default()
            > 100.0,
        "desktop fixture must scroll: {pinned_header}"
    );
    assert_eq!(
        pinned_header["initial"]["mainPaddingLeft"], 32.0,
        "desktop articles should keep a 32px reading gutter: {pinned_header}"
    );
    assert!(
        pinned_header["initial"]["headPaddingTop"]
            .as_f64()
            .unwrap_or_default()
            >= 32.0
            && pinned_header["initial"]["headPaddingBottom"]
                .as_f64()
                .unwrap_or_default()
                >= 14.0
            && pinned_header["initial"]["titleMarginBottom"]
                .as_f64()
                .unwrap_or_default()
                >= 12.0,
        "desktop article headers should have comfortable vertical spacing: {pinned_header}"
    );
    for axis in ["headTop", "titleLeft"] {
        let initial = pinned_header["initial"][axis]
            .as_f64()
            .context("initial pinned header coordinate")?;
        let stuck = pinned_header["stuck"][axis]
            .as_f64()
            .context("stuck pinned header coordinate")?;
        assert!(
            (initial - stuck).abs() <= 1.0,
            "the article header must not shift when it becomes sticky ({axis}): {pinned_header}"
        );
    }
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let article_widths = client
        .execute(
            r#"
      return ['.itemhead', '.body', '.article-footer'].map(selector => {
        const rect = document.querySelector(selector).getBoundingClientRect();
        return {left:rect.left, width:rect.width};
      });
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        article_widths[0], article_widths[1],
        "header and prose must align"
    );
    assert_eq!(
        article_widths[1], article_widths[2],
        "both separators must span the same article width"
    );
    screenshot(client, "desktop-article-expanded").await?;
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,520);setTimeout(done,350);",
            vec![],
        )
        .await?;
    screenshot(client, "desktop-article-condensed").await?;
    client.execute("window.scrollTo(0,0)", vec![]).await?;
    emulate(
        client,
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}),
    )
    .await?;
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let article = client.execute(r#"
      const body = document.querySelector('.body'), head = document.querySelector('.itemhead');
      const image = body.querySelector('img');
      const picture = image.closest('.article-picture');
      const loadedBox = picture.getBoundingClientRect();
      const unloaded = picture.cloneNode(true);
      unloaded.classList.remove('is-loaded');
      unloaded.removeAttribute('data-article-media-bound');
      unloaded.style.removeProperty('--image-preview');
      unloaded.querySelectorAll('source').forEach(source => source.removeAttribute('srcset'));
      unloaded.querySelector('img').removeAttribute('src');
      picture.after(unloaded);
      const unloadedBox = unloaded.getBoundingClientRect();
      unloaded.remove();
      const footer = document.querySelector('.article-footer').getBoundingClientRect();
      const more = document.querySelector('.article-more').getBoundingClientRect();
      const moreCards = Array.from(document.querySelectorAll('.article-more-link'));
      const moreBoxes = moreCards.map(card => card.getBoundingClientRect());
      return {code:body.querySelector('pre code').textContent, link:getComputedStyle(body.querySelector('p a')).display,
        headerMeta:head.querySelector('.meta').textContent.replace(/\s+/g, ' ').trim(),
        imageLoading:image.getAttribute('loading'), imageSource:image.getAttribute('src'), imageClass:image.className,
        imagePriority:image.getAttribute('fetchpriority'), imagePlaceholder:picture.style.getPropertyValue('--image-placeholder'),
        imagePreview:picture.style.getPropertyValue('--image-preview'), pictureLoaded:picture.classList.contains('is-loaded'),
        sourceWidth:picture.querySelector('source')?.getAttribute('width'), sourceHeight:picture.querySelector('source')?.getAttribute('height'),
        sourceSet:picture.querySelector('source')?.getAttribute('srcset'),
        loadedBox:{width:loadedBox.width,height:loadedBox.height}, unloadedBox:{width:unloadedBox.width,height:unloadedBox.height},
        imageWidth:image.naturalWidth, gap:body.getBoundingClientRect().top-head.getBoundingClientRect().bottom,
        mainPaddingLeft:parseFloat(getComputedStyle(document.querySelector('.main')).paddingLeft),
        readingStats:head.querySelector('.reading-stats')?.textContent.replace(/\s+/g, ' ').trim(),
        readingDuration:head.querySelector('.reading-stats time')?.getAttribute('datetime'),
        wordCount:head.querySelector('.reading-stats')?.title,
        publicationTitle:head.querySelector('.dt-published').closest('[data-date-tooltip]')?.title,
        timeTitle:head.querySelector('.dt-published').getAttribute('title'),
        linksTitled:Array.from(head.querySelectorAll('.u-bookmark-of, .discussion')).every(link => link.title === link.href),
        indent:parseFloat(getComputedStyle(body.querySelector('p')).textIndent),
        separateTags:!head.querySelector('.meta .item-tags'), metadataOrder:Array.from(head.querySelector('.meta').children).map(node => node.firstElementChild.className), categoryBelow:!!head.querySelector('.item-tags a[href*="categories/"]'), categoryText:head.querySelector('.meta .category')?.textContent, categoryUrl:head.querySelector('.meta .category a')?.pathname, categoryQuery:new URL(head.querySelector('.meta .category a').href).searchParams.get('q'), leadingRule:getComputedStyle(body.querySelector('hr:first-child')).display,
        outline:getComputedStyle(document.querySelector('main')).outlineStyle,
        highlighted:!!body.querySelector('pre span'), bodyLeft:body.getBoundingClientRect().left, bodyWidth:body.getBoundingClientRect().width,
        footerLeft:footer.left, footerWidth:footer.width, moreLeft:more.left, moreWidth:more.width,
        moreHeadings:[...document.querySelectorAll('.article-more-heading')].map(node => node.textContent),
        moreHeadingSize:parseFloat(getComputedStyle(document.querySelector('.article-more-heading')).fontSize),
        moreTitleSize:parseFloat(getComputedStyle(moreCards[0]).fontSize),
        moreMetadata:moreCards.every(link=>{const card=link.closest('.article-more-card');return ['.meta .domain','.meta .category a','.meta .published-date','.meta .reading-stats','.meta .u-bookmark-of'].every(selector=>card.querySelector(selector))}),
        moreCount:moreCards.length, moreClasses:Array.from(new Set(moreCards.map(card => card.className))).length,
        moreStacked:moreBoxes.length < 2 || moreBoxes[1].top > moreBoxes[0].bottom};
    "#, vec![]).await?;
    assert_eq!(article["code"], "$ z dotfiles\n$ pwd\n/private/dotfiles\n");
    assert_eq!(article["link"], "inline");
    assert!(
        !article["headerMeta"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("published "),
        "article dates should use the same concise label as feed and search: {article}"
    );
    assert_eq!(
        article["metadataOrder"],
        json!([
            "domain",
            "category",
            "published-date",
            "reading-stats",
            "u-bookmark-of"
        ])
    );
    assert_eq!(article["categoryBelow"], false);
    assert_eq!(article["categoryText"], "/engineering");
    assert_eq!(article["categoryUrl"], "/reader/");
    assert_eq!(article["categoryQuery"], "category:\"engineering\"");
    assert_eq!(article["imageLoading"], "eager");
    assert_eq!(article["imagePriority"], "high");
    assert_eq!(article["imageClass"], "progressive-image");
    assert_eq!(article["imagePlaceholder"], "#315d76");
    assert!(
        article["imagePreview"]
            .as_str()
            .unwrap_or_default()
            .contains("data:image/png;base64,"),
        "{article}"
    );
    assert_eq!(article["pictureLoaded"], true);
    assert_eq!(article["sourceWidth"], Value::Null);
    assert_eq!(article["sourceHeight"], Value::Null);
    assert_eq!(
        article["sourceSet"],
        Value::Null,
        "a partial srcset must not make wide screens upscale a thumbnail: {article}"
    );
    assert_eq!(article["imageWidth"], 640);
    for axis in ["width", "height"] {
        let loaded = article["loadedBox"][axis].as_f64().unwrap_or_default();
        let unloaded = article["unloadedBox"][axis].as_f64().unwrap_or_default();
        assert!(
            loaded > 0.0 && (loaded - unloaded).abs() <= 0.5,
            "reserved image {axis} must match its loaded box: {article}"
        );
    }
    assert!(
        article["imageSource"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assets/images/"),
        "{article}"
    );
    assert_eq!(article["highlighted"], true);
    assert_eq!(article["linksTitled"], true);
    assert_eq!(article["separateTags"], true);
    assert!(
        article["publicationTitle"]
            .as_str()
            .is_some_and(|value| value.contains("2026"))
    );
    assert_eq!(article["timeTitle"], Value::Null);
    assert!(
        article["publicationTitle"]
            .as_str()
            .is_some_and(|title| title.lines().count() == 2
                && title.contains("Published:")
                && title.contains("Updated:")),
        "both dates belong in the publication tooltip: {article}"
    );
    assert!(
        !article["headerMeta"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("updated"),
        "updated timestamps must stay out of visible metadata: {article}"
    );
    assert_eq!(article["leadingRule"], "none");
    assert!(
        article["indent"].as_f64().unwrap_or_default() == 0.0,
        "paragraph indentation is off by default: {article}"
    );
    assert!(
        article["wordCount"]
            .as_str()
            .is_some_and(|text| text.ends_with(" words")),
        "word count should be available on hover: {article}"
    );
    assert_eq!(
        article["mainPaddingLeft"], 20.0,
        "mobile articles should keep a 20px reading gutter: {article}"
    );
    assert!(
        article["readingStats"]
            .as_str()
            .is_some_and(|text| !text.contains("words") && text.ends_with(" min read")),
        "reading time should appear without the word count: {article}"
    );
    assert!(
        article["readingDuration"]
            .as_str()
            .is_some_and(|duration| duration.starts_with("PT")
                && (duration.ends_with('M') || duration.ends_with('S'))),
        "reading time should carry a machine-readable duration: {article}"
    );
    assert!(
        article["gap"].as_f64().unwrap_or_default() >= 20.0,
        "{article}"
    );
    assert!(
        article["outline"] == "none" || article["outline"] == "hidden",
        "{article}"
    );
    assert!(
        (article["bodyLeft"].as_f64().unwrap_or_default()
            - article["footerLeft"].as_f64().unwrap_or_default())
        .abs()
            <= 1.0
            && (article["bodyWidth"].as_f64().unwrap_or_default()
                - article["footerWidth"].as_f64().unwrap_or_default())
            .abs()
                <= 1.0,
        "article content and continuation cards must share the same measure: {article}"
    );
    assert_eq!(article["moreCount"], 2);
    assert_eq!(
        article["moreHeadings"],
        json!(["Coming next", "Discover more"])
    );
    assert!(article["moreHeadingSize"].as_f64().unwrap_or_default() >= 16.0);
    assert!(article["moreTitleSize"].as_f64().unwrap_or_default() >= 16.0);
    assert_eq!(article["moreMetadata"], true);
    assert_eq!(article["moreClasses"], 1);
    assert_eq!(article["moreStacked"], true);
    client
        .execute_async(
            "const done=arguments[arguments.length-1];window.scrollTo(0,0);setTimeout(done,350);",
            vec![],
        )
        .await?;
    let sticky_head = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1];
      window.scrollTo(0, 0);
      requestAnimationFrame(function () {
        const head = document.querySelector('.itemhead');
        const title = head.querySelector('h1');
        const initial = {top:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,
          titleLeft:title.getBoundingClientRect().left,
          topBarBottom:document.querySelector('.top').getBoundingClientRect().bottom,
          paddingTop:parseFloat(getComputedStyle(head).paddingTop),
          position:getComputedStyle(head).position};
        window.scrollTo(0, 520);
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            const stuck = {top:head.getBoundingClientRect().top,titleTop:title.getBoundingClientRect().top,
              titleLeft:title.getBoundingClientRect().left,scrollY:window.scrollY};
            window.scrollTo(0, 0);
            requestAnimationFrame(function () { done({initial,stuck}); });
          });
        });
      });
    "#,
            vec![],
        )
        .await?;
    assert_eq!(sticky_head["initial"]["position"], "sticky");
    assert_eq!(
        sticky_head["initial"]["paddingTop"], 24.0,
        "mobile article headers should retain a 24px top gutter: {sticky_head}"
    );
    assert!(
        (sticky_head["initial"]["top"].as_f64().unwrap_or(f64::MAX)
            - sticky_head["initial"]["topBarBottom"]
                .as_f64()
                .unwrap_or_default())
        .abs()
            <= 1.0,
        "the article title and metadata must remain fixed immediately below the mobile top bar: {sticky_head}"
    );
    for axis in ["top", "titleLeft"] {
        let initial = sticky_head["initial"][axis]
            .as_f64()
            .context("initial mobile sticky-header coordinate")?;
        let stuck = sticky_head["stuck"][axis]
            .as_f64()
            .context("scrolled mobile sticky-header coordinate")?;
        assert!(
            (initial - stuck).abs() <= 1.0,
            "the mobile article header must not jump while becoming sticky: {sticky_head}"
        );
    }
    let reading_header = client.execute_async(r#"
      const done = arguments[arguments.length - 1], head = document.querySelector('.itemhead');
      const tags = head.querySelector('.item-tags');
      async function sample(y) {
        window.scrollTo(0,y);
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        const title=head.querySelector('.itemhead-title');
        return {size:parseFloat(getComputedStyle(title).fontSize) * new DOMMatrixReadOnly(getComputedStyle(title).transform).a,
          opacity:parseFloat(getComputedStyle(tags).opacity), height:tags.getBoundingClientRect().height,
          metadata:getComputedStyle(head.querySelector('.meta')).display,
          separate:tags.getBoundingClientRect().top >= head.querySelector('.meta').getBoundingClientRect().bottom,
          offset:parseFloat(getComputedStyle(document.documentElement).scrollPaddingTop), bottom:head.getBoundingClientRect().bottom};
      }
      (async () => done({expanded:await sample(0),quarter:await sample(40),half:await sample(80),
        condensed:await sample(520),reverseHalf:await sample(80),restored:await sample(0)}))();
    "#, vec![]).await?;
    assert_eq!(reading_header["expanded"]["separate"], true);
    for (before, after) in [
        ("expanded", "quarter"),
        ("quarter", "half"),
        ("half", "condensed"),
    ] {
        for field in ["size", "opacity", "height"] {
            assert!(
                reading_header[before][field].as_f64() > reading_header[after][field].as_f64(),
                "header must fold continuously ({field}): {reading_header}"
            );
        }
    }
    assert_eq!(
        reading_header["half"]["size"],
        reading_header["reverseHalf"]["size"]
    );
    assert_eq!(
        reading_header["expanded"]["size"],
        reading_header["restored"]["size"]
    );
    assert_eq!(reading_header["condensed"]["height"], 0.0);
    assert_ne!(reading_header["condensed"]["metadata"], "none");
    assert!(
        reading_header["condensed"]["offset"].as_f64()
            > reading_header["condensed"]["bottom"].as_f64()
    );
    let title_fold = client.execute_async(r#"
      const done=arguments[arguments.length-1], head=document.querySelector('.itemhead');
      const title=head.querySelector('.itemhead-title') || head.querySelector('h1');
      const original=title.textContent;
      head.style.width='550px';
      title.textContent='Proactive cyber defense for governments and enterprises';
      const frame=()=>new Promise(resolve=>requestAnimationFrame(resolve));
      (async()=>{
        window.scrollTo(0,0); window.dispatchEvent(new Event('resize'));
        await frame(); await frame(); await frame();
        const samples=[];
        for(let y=0;y<=160;y+=4){
          window.scrollTo(0,y); await frame(); await frame();
          samples.push(head.getBoundingClientRect().height);
        }
        const largestStep=Math.max(...samples.slice(1).map((height,i)=>Math.abs(height-samples[i])));
        title.textContent=original; head.style.removeProperty('width'); window.scrollTo(0,0); window.dispatchEvent(new Event('resize'));
        await frame(); await frame(); await frame();
        done({largestStep,expanded:samples[0],folded:samples.at(-1)});
      })();
    "#,vec![]).await?;
    assert!(
        title_fold["largestStep"].as_f64().unwrap_or(f64::MAX) < 4.0,
        "a wrapping title must not make the folding header jump: {title_fold}"
    );
    assert!(title_fold["expanded"].as_f64() > title_fold["folded"].as_f64());
    key(client, "G").await?;
    wait_for(client,"scrollY > 100 && Math.abs(scrollY + innerHeight - document.documentElement.scrollHeight) < 2").await?;
    key(client, "g").await?;
    key(client, "g").await?;
    wait_for(client, "scrollY === 0").await?;
    let reading_progress = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1], head = document.querySelector('.itemhead');
      const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
      async function sample(fraction) {
        window.scrollTo(0, 200);
        await frame(); await frame(); await frame();
        window.scrollTo(0, fraction * (document.documentElement.scrollHeight - innerHeight));
        await frame(); await frame(); await frame();
        const indicator = head.querySelector('.itemhead-progress'), line = getComputedStyle(indicator);
        return {progress:new DOMMatrixReadOnly(line.transform).a,
          fill:indicator.getBoundingClientRect().width / head.getBoundingClientRect().width, transition:line.transitionDuration,
          expected:scrollY / (document.documentElement.scrollHeight - innerHeight)};
      }
      (async () => done({top:await sample(0),quarter:await sample(.25),half:await sample(.5),
        bottom:await sample(1),reverse:await sample(.5),restored:await sample(0)}))();
    "#,
            vec![],
        )
        .await?;
    for (sample, expected) in [
        ("top", 0.0),
        ("quarter", 0.25),
        ("half", 0.5),
        ("bottom", 1.0),
        ("reverse", 0.5),
        ("restored", 0.0),
    ] {
        let progress = reading_progress[sample]["progress"]
            .as_f64()
            .context("reading progress")?;
        let fill = reading_progress[sample]["fill"]
            .as_f64()
            .context("progress line scale")?;
        assert!(
            (progress - expected).abs() < 0.002 && (fill - progress).abs() < 0.00001,
            "the header separator must fill linearly in both directions: {reading_progress}"
        );
        assert_eq!(reading_progress[sample]["transition"], "0s");
    }
    client.execute("window.scrollTo(0,520)", vec![]).await?;
    let header_reads = client
        .execute_async(
            r#"
      const done = arguments[arguments.length - 1], head = document.querySelector('.itemhead');
      const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
      (async () => {
        await frame(); await frame(); await frame();
        const original = head.getBoundingClientRect;
        let reads = 0;
        head.getBoundingClientRect = function () { reads++; return original.call(this); };
        for (const y of [600, 700, 800, 900]) { scrollTo(0,y); await frame(); await frame(); }
        head.getBoundingClientRect = original;
        scrollTo(0,520);
        done(reads);
      })();
    "#,
            vec![],
        )
        .await?;
    assert_eq!(
        header_reads, 0,
        "folded scrolling should reuse measured header geometry"
    );
    screenshot(client, "mobile-article-folded").await?;
    client.execute("window.scrollTo(0,0)", vec![]).await?;
    wait_for(client, "scrollY === 0").await?;
    screenshot(client, "mobile-article").await?;
    client.back().await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    wait_for(
        client,
        "!!document.activeElement.matches('[data-row-open]')",
    )
    .await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn article_keyboard_shortcuts_and_feed_paging() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::new()?;
    let client = browser_client().await?;
    let result = catch_panics(article_keyboard_contracts(&client, &fixture)).await;
    report_failure(&client, "article-keyboard", &result).await;
    finish(client, result).await
}

async fn article_keyboard_contracts(client: &Client, fixture: &Fixture) -> Result<()> {
    phone_session(client).await?;
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    client
        .execute("localStorage.setItem('aggr:theme','light')", vec![])
        .await?;
    client.goto(&fixture.base).await?;
    wait_booted_with(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;
    // The newest article: where the feed cursor starts and where both keyboard flows return.
    let first = client
        .execute(
            "return document.querySelector('[data-row-open]').href",
            vec![],
        )
        .await?;
    client
        .goto(first.as_str().context("selected article URL")?)
        .await?;
    wait_for(client, "!!document.querySelector('article.item')").await?;
    client
        .execute(
            "window.__aggrOpened=[];window.open=function(url){window.__aggrOpened.push(url);return {};};",
            vec![],
        )
        .await?;
    for key_name in ["O", "H", "R", "X"] {
        key(client, key_name).await?;
    }
    let external_shortcuts = client
        .execute(
            r#"
      return {enabled:window.AGGR.discussions.map(network => network.shortcut),
        opened:window.__aggrOpened.map(value => { const url=new URL(value); return {host:url.host,query:url.search}; })};
    "#,
            vec![],
        )
        .await?;
    assert_eq!(external_shortcuts["enabled"], json!(["H", "R"]));
    assert_eq!(
        external_shortcuts["opened"].as_array().map(Vec::len),
        Some(3)
    );
    assert_eq!(external_shortcuts["opened"][0]["host"], "publisher.invalid");
    assert_eq!(external_shortcuts["opened"][1]["host"], "hn.algolia.com");
    assert!(
        external_shortcuts["opened"][1]["query"]
            .as_str()
            .unwrap_or_default()
            .contains("publisher.invalid%2Fstory-45"),
        "{external_shortcuts}"
    );
    assert_eq!(external_shortcuts["opened"][2]["host"], "www.reddit.com");
    let scroll_shortcuts = client
        .execute(
            r#"
      const movements = [], prevented = {};
      const scrollBy = window.scrollBy;
      window.scrollBy = (options) => movements.push(options.top);
      function press(label, key, options = {}, target = document.body) {
        const event = new KeyboardEvent('keydown', {key, bubbles:true, cancelable:true, ...options});
        target.dispatchEvent(event);
        prevented[label] = event.defaultPrevented;
      }
      press('d', 'd'); press('ctrlD', 'd', {ctrlKey:true});
      press('u', 'u'); press('ctrlU', 'u', {ctrlKey:true});
      press('ctrlE', 'e', {ctrlKey:true}); press('ctrlY', 'y', {ctrlKey:true});
      press('metaD', 'd', {metaKey:true}); press('ctrlShiftD', 'D', {ctrlKey:true, shiftKey:true});
      const input = document.createElement('textarea'); document.body.append(input);
      press('editing', 'u', {ctrlKey:true}, input); input.remove();
      const dialog = document.querySelector('#shortcut-help'); dialog.showModal();
      press('dialog', 'd', {ctrlKey:true}); dialog.close(); document.querySelector('#swup').focus({preventScroll:true});
      const preference = window.AGGRPreferences.values['single-key-shortcuts'];
      window.AGGRPreferences.values['single-key-shortcuts'] = false;
      press('disabledD', 'd'); press('enabledCtrlD', 'd', {ctrlKey:true});
      window.AGGRPreferences.values['single-key-shortcuts'] = preference;
      window.scrollBy = scrollBy;
      const line = parseFloat(getComputedStyle(document.querySelector('.body')).lineHeight);
      const head = document.querySelector('.itemhead').getBoundingClientRect();
      const bottom = 0;
      return {movements, prevented, line, halfPage:Math.min(line * 10, Math.max(line, innerHeight - head.bottom - bottom) * 0.5)};
    "#,
            vec![],
        )
        .await?;
    let half_page = scroll_shortcuts["halfPage"]
        .as_f64()
        .context("reading scroll distance")?;
    let line = scroll_shortcuts["line"]
        .as_f64()
        .context("reading line height")?;
    assert_eq!(
        scroll_shortcuts["movements"],
        json!([
            half_page, half_page, -half_page, -half_page, line, -line, half_page
        ])
    );
    for name in ["d", "ctrlD", "u", "ctrlU", "ctrlE", "ctrlY", "enabledCtrlD"] {
        assert_eq!(
            scroll_shortcuts["prevented"][name], true,
            "{name}: {scroll_shortcuts}"
        );
    }
    for name in ["metaD", "ctrlShiftD", "editing", "dialog", "disabledD"] {
        assert_eq!(
            scroll_shortcuts["prevented"][name], false,
            "{name}: {scroll_shortcuts}"
        );
    }
    let article_paths = client
        .execute(
            r#"
      const article = document.querySelector('article.item');
      return {current:location.pathname,older:new URL(article.dataset.nextUrl,new URL(document.getElementById('aggr-page').dataset.root,location.href)).pathname};
    "#,
            vec![],
        )
        .await?;
    let current_path = article_paths["current"]
        .as_str()
        .context("current article path")?
        .to_string();
    let older_path = article_paths["older"]
        .as_str()
        .context("older article path")?
        .to_string();
    for key_name in ["h", "l", "ArrowLeft", "ArrowRight"] {
        key(client, key_name).await?;
        assert_eq!(
            client.current_url().await?.path(),
            current_path,
            "{key_name} must not navigate between articles"
        );
    }
    key(client, "k").await?;
    assert_eq!(
        client.current_url().await?.path(),
        current_path,
        "k at the newest-article boundary must be a no-op"
    );
    client
        .execute("document.querySelector('.body a').focus()", vec![])
        .await?;
    assert_eq!(
        client
            .execute(
                "return document.activeElement.closest('.body') !== null",
                vec![]
            )
            .await?,
        true,
        "the keyboard contract must be exercised from a focused article link"
    );
    key(client, "j").await?;
    wait_for(
        client,
        &format!(
            "location.pathname === {} && !!document.querySelector('article.item')?.dataset.previousUrl && !document.documentElement.classList.contains('is-changing')",
            serde_json::to_string(&older_path)?
        ),
    )
    .await?;
    assert_eq!(
        client
            .execute(
                "return new URL(document.querySelector('article.item').dataset.previousUrl,new URL(document.getElementById('aggr-page').dataset.root,location.href)).pathname",
                vec![]
            )
            .await?,
        current_path,
        "the older article must link back to the article opened from the feed"
    );
    client
        .execute("document.querySelector('.itemhead a').focus()", vec![])
        .await?;
    key(client, "k").await?;
    wait_for(
        client,
        &format!(
            "location.pathname === {} && !!document.querySelector('article.item') && !document.documentElement.classList.contains('is-changing')",
            serde_json::to_string(&current_path)?
        ),
    )
    .await?;
    client.goto(&fixture.base).await?;
    wait_for(
        client,
        "document.querySelectorAll('[data-row-open]').length === 3",
    )
    .await?;

    for key_name in ["h", "l", "ArrowLeft", "ArrowRight"] {
        key(client, key_name).await?;
        assert_eq!(
            client.current_url().await?.path(),
            "/reader/",
            "{key_name} must not turn list pages"
        );
    }
    client
        .find(Locator::Css("[data-page-next]"))
        .await?
        .click()
        .await?;
    wait_for(client, "location.pathname.includes('/page/2/')").await?;
    wait_for(client, "document.querySelector('[data-row-open]').href.includes('story-42/') && !document.documentElement.classList.contains('is-changing')").await?;
    key(client, "j").await?;
    client.back().await?;
    wait_for(
        client,
        "location.pathname === '/reader/' && document.querySelector('[data-row-open]')?.href.includes('story-45/') && !!document.activeElement.matches('[data-row-open]') && !document.documentElement.classList.contains('is-changing')",
    )
    .await?;
    assert_eq!(
        client
            .execute("return document.activeElement.href", vec![])
            .await?,
        first
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn interactive_originals_and_build_time_code_labels() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let archive = fixture.directory.path().join(".aggr/data");
    let relative = "items/example/2026/09/2026-09-09-interactive.md";
    std::fs::write(
        archive.join(relative),
        format!(
            "---\ntitle: Interactive diagrams\nlink: {}interactive.html\nsource: example\npublished: 2026-09-09T12:00:00Z\nfirst_seen: 2026-09-09T12:00:00Z\ncontent: extracted\nextra:\n  content:interactive: true\n---\n\nRetained explanation.\n\n```rust\nfn main() {{ println!(\"hello\"); }}\n```\n",
            fixture.base
        ),
    )?;
    git(&archive, &["add", relative])?;
    git(&archive, &["commit", "-qm", "fixture interactive page"])?;
    fixture.build()?;
    anyhow::ensure!(
        std::fs::read_to_string(
            fixture
                .out
                .join("items/example/2026-09-09-interactive/index.html")
        )?
        .contains("data-interactive-embed"),
        "generated interactive fixture must include its original-link fallback"
    );
    std::fs::write(
        fixture.out.join("interactive.html"),
        r#"<!doctype html><title>Live diagrams</title><canvas width="640" height="320"></canvas><script>let blocked=false;try{parent.document.body}catch(error){blocked=error.name==='SecurityError'}parent.postMessage({interactiveDemo:true,blocked},'*');</script>"#,
    )?;
    let client = browser_client().await?;
    let result = async {
        emulate(&client,"Page.addScriptToEvaluateOnNewDocument",json!({"source":r#"
          addEventListener('message',event=>{if(event.data?.interactiveDemo)window.interactiveReport={...event.data,origin:event.origin}});
          const observer=new MutationObserver(()=>{
            const box=document.querySelector('.interactive-frame');
            if(!box?.querySelector('iframe'))return;
            const r=box.getBoundingClientRect();
            window.interactiveInitialGeometry={width:r.width,height:r.height};
            observer.disconnect();
          });
          observer.observe(document,{childList:true,subtree:true});
        "#})).await?;
        for width in [1280,390] {
            emulate(&client,"Emulation.setDeviceMetricsOverride",json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            client.goto(&format!("{}items/example/2026-09-09-interactive/",fixture.base)).await?;
            wait_for(&client,"window.interactiveReport?.blocked===true && document.querySelector('[data-interactive-embed]').hidden").await?;
            let state=client.execute(r#"
              const box=document.querySelector('.interactive-frame'),frame=box.querySelector('iframe'),r=box.getBoundingClientRect(),code=document.querySelector('.code-snippet');
              return {width:r.width,height:r.height,before:window.interactiveInitialGeometry,focused:document.activeElement===frame,frames:box.querySelectorAll('iframe').length,sandbox:frame.getAttribute('sandbox'),referrer:frame.referrerPolicy,origin:window.interactiveReport.origin,language:code.dataset.language,label:getComputedStyle(code,'::before').content,labelColor:getComputedStyle(code,'::before').color,source:code.textContent,highlighted:!!code.querySelector('[class*=syntax-keyword],[class*=syntax-storage]'),overflow:document.documentElement.scrollWidth>document.documentElement.clientWidth+1};
            "#,vec![]).await?;
            anyhow::ensure!(state["width"]==state["before"]["width"] && state["height"]==state["before"]["height"] && state["overflow"]==false,"automatic interactive loading preserves desktop/mobile geometry: {state}");
            anyhow::ensure!(state["focused"]==false,"automatic interactive loading must not move keyboard focus: {state}");
            anyhow::ensure!(state["sandbox"]=="allow-scripts" && state["referrer"]=="no-referrer" && state["origin"]=="null" && state["frames"]==1,"interactive source keeps an opaque origin and minimal permissions: {state}");
            anyhow::ensure!(state["language"]=="Rust" && state["label"]=="\"Rust\"" && state["highlighted"]==true && state["source"]=="fn main() { println!(\"hello\"); }\n","build-time labels and syntax colors preserve copied code: {state}");
        }
        Ok(())
    }.await;
    report_failure(&client, "interactive", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn recommendation_cards_and_navigation_layout() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    let result = async {
        for width in [1280, 390] {
            emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600})).await?;
            client.goto(&format!("{}items/example/2026-09-01-story-20/", fixture.base)).await?;
            wait_for(&client, "document.querySelectorAll('.article-more-section').length===2").await?;
            let layout = client.execute(r#"
              const sections=[...document.querySelectorAll('.article-more-section')];
              const cards=sections.map(section=>section.querySelector('.article-more-card'));
              cards[0].querySelector('.preview-media')?.remove();
              cards[0].querySelector('.title').textContent='Short title';
              cards[1].querySelector('.title').textContent='A much longer recommendation title that wraps onto several lines and needs more natural reading space';
              const boxes=cards.map(card=>{const r=card.getBoundingClientRect();return {top:r.top,bottom:r.bottom,height:r.height}});
              const unusedHeight=cards[0].querySelector('.row-content').getBoundingClientRect().height-cards[0].querySelector('.row-copy').getBoundingClientRect().height;
              return {boxes,unusedHeight,headingMargin:parseFloat(getComputedStyle(document.querySelector('.itemhead')).marginBottom),padding:cards.map(card=>getComputedStyle(card).paddingTop)};
            "#, vec![]).await?;
            let first=&layout["boxes"][0];
            let second=&layout["boxes"][1];
            anyhow::ensure!(second["top"].as_f64()>first["bottom"].as_f64(), "recommendations stack vertically at every viewport width: {layout}");
            anyhow::ensure!(layout["unusedHeight"].as_f64().unwrap().abs()<1.0, "text-only recommendations fit their content without reserving absent previews: {layout}");
            anyhow::ensure!(layout["headingMargin"].as_f64().unwrap()>=if width>600 {36.0} else {28.0}, "article header needs a little breathing room: {layout}");
            anyhow::ensure!(layout["padding"].as_array().unwrap().iter().all(|value|value=="12.8px"), "card padding stays compact: {layout}");
            wait_booted(&client).await?;
            for index in [0, 1] {
                let point = client.execute(r#"
                  const card=document.querySelectorAll('.article-more-card')[arguments[0]];
                  card.scrollIntoView({block:'center'});
                  const r=card.getBoundingClientRect();
                  return {x:r.left+4,y:r.top+4,background:getComputedStyle(card).backgroundColor};
                "#, vec![json!(index)]).await?;
                emulate(&client,"Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":point["x"],"y":point["y"]})).await?;
                let hover=client.execute(r#"
                  const card=document.querySelectorAll('.article-more-card')[arguments[0]];
                  return {background:getComputedStyle(card).backgroundColor,cursor:getComputedStyle(card).cursor,decoration:getComputedStyle(card.querySelector('.title')).textDecorationLine};
                "#,vec![json!(index)]).await?;
                anyhow::ensure!(hover["background"]==point["background"] && hover["cursor"]=="pointer" && hover["decoration"]=="underline", "card space underlines the title without changing its surface: {hover}");
                let clicks=client.execute(r#"
                  const card=document.querySelectorAll('.article-more-card')[arguments[0]],title=card.querySelector('.title'),forwarded=[];
                  const capture=event=>{forwarded.push({button:event.button,ctrl:event.ctrlKey,shift:event.shiftKey});event.preventDefault();event.stopPropagation()};
                  title.addEventListener('click',capture);
                  card.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true,ctrlKey:true,shiftKey:true}));
                  card.dispatchEvent(new MouseEvent('auxclick',{bubbles:true,cancelable:true,button:1}));
                  const range=document.createRange();range.selectNodeContents(title);getSelection().addRange(range);
                  card.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
                  getSelection().removeAllRanges();
                  title.removeEventListener('click',capture);
                  return forwarded;
                "#,vec![json!(index)]).await?;
                anyhow::ensure!(clicks==json!([{"button":0,"ctrl":true,"shift":true},{"button":1,"ctrl":false,"shift":false}]), "card clicks preserve modifiers and leave text selection alone: {clicks}");
            }
            let destination=client.execute("const card=document.querySelector('.article-more-card');const href=card.querySelector('.title').href;card.click();return href",vec![]).await?;
            wait_for(&client,&format!("location.href==={destination}")).await?;
            client.goto(&format!("{}categories/", fixture.base)).await?;
            let nav=client.execute(r#"
              const links=[...document.querySelectorAll('.nav .menu-link')];
              return {regular:links.every(link=>getComputedStyle(link).fontWeight==='400'),prominent:links.every(link=>getComputedStyle(link).opacity==='1'),selected:getComputedStyle(document.querySelector('.nav a[aria-current]')).textDecorationLine,pipe:document.querySelector('.nav-primary .nav-separator')?.textContent};
            "#,vec![]).await?;
            anyhow::ensure!(nav["regular"]==true && nav["prominent"]==true && nav["selected"]=="underline" && nav["pipe"]=="|", "navigation uses regular prominent labels and a selected underline: {nav}");
        }
        Ok(())
    }.await;
    report_failure(&client, "recommendation-cards", &result).await;
    finish(client, result).await
}

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn title_metadata_matches_feed_search_and_article() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let client = browser_client().await?;
    emulate(
        &client,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":"Date.now=()=>Date.parse('2026-09-03T01:00:00Z');"}),
    )
    .await?;
    client.goto(&fixture.base).await?;
    let result = async {
        for width in [1280, 390] {
            emulate(
                &client,
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":width<600}),
            )
            .await?;
            emulate(
                &client,
                "Emulation.setTouchEmulationEnabled",
                json!({"enabled":width<600,"maxTouchPoints":1}),
            )
            .await?;
            for format in ["relative", "iso"] {
                client
                    .execute(
                        "localStorage.setItem('aggr:date-format',arguments[0])",
                        vec![json!(format)],
                    )
                    .await?;
                metadata_contracts(&client, &fixture, width)
                    .await
                    .with_context(|| format!("metadata at {width}px with {format} dates"))?;
                let expected=if format=="relative" {"4h ago"} else {"2026-09-02"};
                anyhow::ensure!(client.execute("return document.querySelector('.itemhead .dt-published').textContent.trim()",vec![]).await?==expected,"the {format} preference must be applied before comparing metadata");
            }
        }
        Ok(())
    }
    .await;
    report_failure(&client, "title-metadata", &result).await;
    finish(client, result).await
}

async fn visible_metadata(client: &Client, selector: &str) -> Result<Value> {
    Ok(client.execute(r#"
      return [...document.querySelector(arguments[0]).children]
        .map(field=>{
          const copy=field.cloneNode(true);copy.querySelectorAll('.sr-only').forEach(node=>node.remove());
          const time=field.querySelector('time')?.dateTime || null;
          return {text:copy.textContent.replace(/\s+/g,' ').trim(),links:[...field.querySelectorAll('a')].map(link=>link.href),date:time && !time.startsWith('P') ? new Date(time).toISOString() : time};
        });
    "#,vec![json!(selector)]).await?)
}

async fn metadata_layout(client: &Client, selector: &str) -> Result<Value> {
    Ok(client.execute(r#"
      const meta=document.querySelector(arguments[0]);
      const fields=[...meta.children];
      const style=node=>{const css=getComputedStyle(node);return {
        font:css.fontFamily,size:css.fontSize,weight:css.fontWeight,line:css.lineHeight,
        spacing:css.letterSpacing,color:css.color,align:css.alignItems
      };};
      const textBox=node=>{const range=document.createRange();range.selectNodeContents(node);return range.getBoundingClientRect();};
      const date=meta.querySelector('.published-date'), dateText=textBox(date.querySelector('time'));
      const next=meta.querySelector('.reading-stats'), nextText=textBox(next);
      const source=meta.querySelector('.domain'), resolved=source?.querySelector('.source-resolved'), via=source?.querySelector('em');
      const probe=document.createElement('span');document.body.append(probe);
      probe.style.color='var(--warm)';const warm=getComputedStyle(probe).color;
      probe.style.color='var(--muted)';const muted=getComputedStyle(probe).color;probe.remove();
      return {
        source:{resolved:resolved ? getComputedStyle(resolved).color:null,via:via ? getComputedStyle(via).color:null,warm,muted},
        typography:style(meta),
        fields:fields.map((field,index)=>{
          const css=getComputedStyle(field),separator=getComputedStyle(field,'::before');
          return {typography:style(field),display:css.display,height:field.getBoundingClientRect().height,
            separator:index ? {content:separator.content,left:separator.marginLeft,right:separator.marginRight}:null};
        }),
        date:{text:date.textContent.trim(),trailingSpace:date.getBoundingClientRect().right-dateText.right,
          gap:Math.abs(dateText.top-nextText.top)<2 ? nextText.left-dateText.right:null,
          fontSize:parseFloat(getComputedStyle(date).fontSize)}
      };
    "#,vec![json!(selector)]).await?)
}

async fn metadata_contracts(client: &Client, fixture: &Fixture, width: u32) -> Result<()> {
    client.goto(&fixture.base).await?;
    wait_booted(client).await?;
    let feed = visible_metadata(client, ".row .meta").await?;
    let feed_layout = metadata_layout(client, ".row .meta").await?;
    anyhow::ensure!(
        client.execute("return !!document.querySelector('.row .domain .source-resolved') && !!document.querySelector('.row .reading-stats') && !document.querySelector('.row .tag')",vec![]).await? == true,
        "feed metadata should show its source and reading time, with tags reserved for the article"
    );
    client
        .goto(&format!("{}?q=A%20long%20article%20title", fixture.base))
        .await?;
    wait_for(
        client,
        "document.querySelector('#list .row [data-row-open]')?.href.includes('story-45/')",
    )
    .await?;
    let search = visible_metadata(client, "#list .row .meta").await?;
    let search_layout = metadata_layout(client, "#list .row .meta").await?;
    anyhow::ensure!(
        client
            .execute("return !document.querySelector('#list .row .tag')", vec![])
            .await?
            == true,
        "search metadata must keep tags on the article"
    );
    anyhow::ensure!(
        feed == search,
        "feed/search metadata differ: feed={feed}, search={search}"
    );
    client
        .goto(&format!(
            "{}items/example/2026-09-01-story-45/",
            fixture.base
        ))
        .await?;
    wait_booted_with(client, "!!document.querySelector('.itemhead .meta')").await?;
    let article = visible_metadata(client, ".itemhead .meta").await?;
    let article_layout = metadata_layout(client, ".itemhead .meta").await?;
    anyhow::ensure!(
        client
            .execute("return !!document.querySelector('.item-tags .tag')", vec![])
            .await?
            == true,
        "article tags must remain visible below its metadata"
    );
    let tags = client.execute("const tag=getComputedStyle(document.querySelector('.item-tags .tag'));const meta=getComputedStyle(document.querySelector('.itemhead .meta'));return {color:tag.color,metadataColor:meta.color,font:tag.fontSize,metadataFont:meta.fontSize,background:tag.backgroundColor,border:tag.borderTopWidth,weight:tag.fontWeight}", vec![]).await?;
    anyhow::ensure!(
        tags["color"] == tags["metadataColor"]
            && tags["font"] == tags["metadataFont"]
            && tags["background"] == "rgba(0, 0, 0, 0)"
            && tags["border"] == "0px"
            && tags["weight"] == "400",
        "tags must share the plain muted metadata style: {tags}"
    );
    // Compact feed rows deliberately stack and shrink their metadata at and below 40rem; the
    // article keeps the full treatment. Feed and search rows share it at every width.
    anyhow::ensure!(
        width <= 640
            || (feed_layout["typography"] == article_layout["typography"]
                && feed_layout["fields"] == article_layout["fields"]),
        "shared metadata geometry and typography differ: feed={feed_layout}, article={article_layout}"
    );
    anyhow::ensure!(
        feed_layout["typography"] == search_layout["typography"]
            && feed_layout["fields"] == search_layout["fields"],
        "shared metadata geometry and typography differ: feed={feed_layout}, search={search_layout}"
    );
    for (kind, layout) in [
        ("feed", &feed_layout),
        ("search", &search_layout),
        ("article", &article_layout),
    ] {
        anyhow::ensure!(
            layout["source"]["resolved"] == layout["source"]["warm"]
                && (layout["source"]["via"].is_null()
                    || layout["source"]["via"] == layout["source"]["muted"]),
            "{kind} source should be orange while via text remains muted: {layout}"
        );
        anyhow::ensure!(
            layout["date"]["trailingSpace"]
                .as_f64()
                .is_some_and(|space| space.abs() <= 1.0),
            "{kind} date reserves visible unused spacing: {layout}"
        );
        if let Some(gap) = layout["date"]["gap"].as_f64() {
            anyhow::ensure!(
                (0.0..=layout["date"]["fontSize"].as_f64().unwrap_or_default() * 1.5)
                    .contains(&gap),
                "{kind} date-to-reading-time gap is excessive: {layout}"
            );
        }
    }
    anyhow::ensure!(
        feed == article,
        "feed/article metadata differ: feed={feed}, article={article}"
    );
    Ok(())
}
