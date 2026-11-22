//! Diagnostic client benchmark with fresh sessions, cold/warm PWA visits and offline startup.
//! AGGR_PERFORMANCE_RUNS selects one to ten samples per environment (default five).
//! AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser_performance -- --ignored

#[allow(dead_code)]
#[path = "browser/harness.rs"]
mod harness;

use std::path::Path;

use anyhow::Result;
use fantoccini::Locator;
use serde_json::{Value, json};

use harness::{Fixture, browser_client, emulate, finish, screenshot, wait_for};

const OBSERVERS: &str = r#"
window.__aggrPerfErrors=[];
addEventListener('error',e=>window.__aggrPerfErrors.push(e.message));
addEventListener('unhandledrejection',e=>window.__aggrPerfErrors.push(String(e.reason)));
window.__aggrPerf={lcpMs:null,cls:0,longTaskCount:0,longTaskTotalMs:0,longTaskMaxMs:0,eventTimingMaxMs:null};
let clusterStart=-Infinity,clusterEnd=-Infinity,clusterScore=0;
for(const type of ['largest-contentful-paint','layout-shift','longtask','event']){
  if(!PerformanceObserver.supportedEntryTypes.includes(type))continue;
  new PerformanceObserver(list=>{
    for(const entry of list.getEntries()){
      const m=window.__aggrPerf;
      if(type==='largest-contentful-paint')m.lcpMs=entry.startTime;
      if(type==='layout-shift'&&!entry.hadRecentInput){
        if(entry.startTime-clusterEnd>1000||entry.startTime-clusterStart>5000){clusterStart=entry.startTime;clusterScore=0}
        clusterEnd=entry.startTime;clusterScore+=entry.value;m.cls=Math.max(m.cls,clusterScore);
      }
      if(type==='longtask'){m.longTaskCount++;m.longTaskTotalMs+=entry.duration;m.longTaskMaxMs=Math.max(m.longTaskMaxMs,entry.duration)}
      if(type==='event'&&entry.interactionId)m.eventTimingMaxMs=Math.max(m.eventTimingMaxMs||0,entry.duration);
    }
  }).observe({type,buffered:true,...(type==='event'?{durationThreshold:16}:{})});
}
requestAnimationFrame(function ready(){
  if(document.documentElement.dataset.aggrReady==='true'&&document.querySelector('.search-command'))window.__aggrPerfReadyAt=performance.now();
  else requestAnimationFrame(ready);
});
"#;

const SCENARIO: &str = r#"
const base=arguments[0],offline=arguments[1],done=arguments[arguments.length-1];
(async()=>{
  const until=async check=>{const started=performance.now();while(!check()){if(performance.now()-started>30000)throw Error('timed out waiting for performance scenario');await new Promise(requestAnimationFrame)}};
  await until(()=>window.__aggrPerfReadyAt&&performance.getEntriesByName('first-contentful-paint').length);
  await until(()=>[...document.images].every(image=>image.complete));
  await new Promise(resolve=>setTimeout(resolve,250));
  const result={...window.__aggrPerf,initialReadyMs:window.__aggrPerfReadyAt,fcpMs:performance.getEntriesByName('first-contentful-paint')[0].startTime,
    workerControlled:!!navigator.serviceWorker.controller,transferBytes:performance.getEntriesByType('resource').reduce((total,entry)=>total+entry.transferSize,0),
    resourceCount:performance.getEntriesByType('resource').length,supportedEntryTypes:PerformanceObserver.supportedEntryTypes};
  if(!offline){
    const go=href=>{const link=document.createElement('a');link.href=href;document.body.append(link);link.click();link.remove()};
    let started=performance.now();
    go(new URL('preferences/',base).href);
    await until(()=>document.querySelector('#preferences-controls:not([disabled]) [data-preference]'));
    result.preferencesNavigationMs=performance.now()-started;
    started=performance.now();go(base);
    await until(()=>document.body.dataset.kind==='river'&&document.querySelector('.search-command'));
    result.feedNavigationMs=performance.now()-started;
    const input=document.querySelector('#q');started=performance.now();
    input.value='category:engineering "Reading comfortably"';
    input.setSelectionRange(input.value.length,input.value.length);
    input.dispatchEvent(new Event('input',{bubbles:true}));
    await until(()=>document.querySelector('.search-results .row'));
    result.firstSearchMs=performance.now()-started;
    result.results=document.querySelectorAll('.search-results .row').length;
  }
  result.clientErrors=window.__aggrPerfErrors;
  done(result);
})().catch(error=>done({error:error.stack||String(error),ready:window.__aggrPerfReadyAt,appReady:document.documentElement.dataset.aggrReady,clientErrors:window.__aggrPerfErrors,url:location.href}));
"#;

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_client_performance_metrics() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(true)?;
    let mut measurements = Vec::new();
    let runs = std::env::var("AGGR_PERFORMANCE_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=10).contains(value))
        .unwrap_or(5);
    for (environment, width, mobile, rate, latency, throughput) in [
        ("desktop-local", 1280, false, 1, 0, -1),
        ("mobile-4x-slow4g", 390, true, 4, 120, 200_000),
    ] {
        for run in 0..runs {
            let client = browser_client().await?;
            let measurement: Result<Vec<Value>> = async {
                emulate(&client, "Network.enable", json!({})).await?;
                emulate(&client, "Network.setCacheDisabled", json!({"cacheDisabled": false})).await?;
                emulate(&client, "Network.emulateNetworkConditions", json!({"offline":false,"latency":latency,"downloadThroughput":throughput,"uploadThroughput":throughput})).await?;
                emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":mobile})).await?;
                emulate(&client, "Emulation.setCPUThrottlingRate", json!({"rate":rate})).await?;
                emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source":OBSERVERS})).await?;
                let mut samples = Vec::new();
                for state in ["cold", "warm", "offline"] {
                    if state == "offline" {
                        emulate(&client, "Network.emulateNetworkConditions", json!({"offline":true,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1})).await?;
                    }
                    client.goto(&fixture.base).await?;
                    let mut value = client.execute_async(SCENARIO, vec![json!(fixture.base), json!(state == "offline")]).await?;
                    anyhow::ensure!(value.get("error").is_none(), "performance scenario ({environment}/{run}/{state}): {value}");
                    if state != "offline" {
                        let input = client.find(Locator::Css("#q")).await?;
                        input.click().await?;
                        input.send_keys(" ").await?;
                        value["eventTimingMaxMs"] = client.execute_async(
                            "const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>setTimeout(()=>done(window.__aggrPerf.eventTimingMaxMs),0)))",
                            vec![],
                        ).await?;
                    }
                    value["environment"] = json!(environment);
                    value["run"] = json!(run);
                    value["state"] = json!(state);
                    samples.push(value);
                    if state == "cold" {
                        wait_for(&client, "!!navigator.serviceWorker.controller").await?;
                        // A worker controls only after its required shell has finished installing.
                        // Wait for the visited feed to finish its streaming runtime-cache write too.
                        let cached = client.execute_async(r#"
                            const base=arguments[0],done=arguments[arguments.length-1];
                            (async()=>{for(let i=0;i<300;i++){
                              if(await caches.match(base))return done(true);
                              await new Promise(resolve=>setTimeout(resolve,100));
                            }done(false)})().catch(()=>done(false));
                        "#, vec![json!(fixture.base)]).await?;
                        anyhow::ensure!(cached == json!(true), "visited feed was not cached before warm/offline measurement");
                    }
                }
                Ok(samples)
            }.await;
            if measurement.is_err() {
                let _ = screenshot(&client, "client-performance-failure").await;
            }
            measurements.extend(finish(client, measurement).await?);
        }
    }
    let mut medians = Vec::new();
    for environment in ["desktop-local", "mobile-4x-slow4g"] {
        for state in ["cold", "warm", "offline"] {
            let mut median = json!({"environment":environment,"state":state});
            for metric in [
                "initialReadyMs",
                "fcpMs",
                "lcpMs",
                "cls",
                "longTaskCount",
                "longTaskTotalMs",
                "longTaskMaxMs",
                "eventTimingMaxMs",
                "transferBytes",
                "resourceCount",
                "preferencesNavigationMs",
                "feedNavigationMs",
                "firstSearchMs",
            ] {
                let mut values: Vec<f64> = measurements
                    .iter()
                    .filter(|sample| {
                        sample["environment"] == environment && sample["state"] == state
                    })
                    .filter_map(|sample| sample[metric].as_f64())
                    .collect();
                values.sort_by(f64::total_cmp);
                median[metric] = if values.is_empty() {
                    Value::Null
                } else {
                    json!(values[values.len() / 2])
                };
            }
            eprintln!("client performance median: {median}");
            medians.push(median);
        }
    }
    let report = json!({"durationUnit":"milliseconds","clsUnit":"unitless","transferUnit":"bytes","archiveItems":45,"runsPerEnvironment":runs,
        "scope":"Production client with publisher media enabled. Fresh PWA session per run, then HTTP/service-worker warm visit and cached offline startup. Mobile: 390px viewport, 4x CPU, 120ms latency and 200000 bytes/s. Local fixture only; no pass thresholds. Navigation/search timings use synthetic actions. Event Timing includes a trusted WebDriver search-focus click and keystroke, is reported only when observed, and is not an INP measurement. LCP/CLS and long tasks cover startup through image completion plus 250ms.",
        "medians":medians,"measurements":measurements});
    let artifacts = Path::new("target/browser-artifacts");
    std::fs::create_dir_all(artifacts)?;
    std::fs::write(
        artifacts.join("client-performance.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
