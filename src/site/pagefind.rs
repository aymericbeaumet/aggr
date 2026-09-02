use std::path::Path;

use anyhow::{Context as _, Result};
use pagefind::api::PagefindIndex;

/// Pagefind's API is async and uses blocking workers internally. Site generation is deliberately
/// synchronous, so isolate its small runtime on a thread; this also works when `aggr build` is
/// already running inside the CLI's multithreaded Tokio runtime.
pub fn build(out: &Path) -> Result<()> {
    let site = out.to_path_buf();
    std::thread::spawn(move || -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("starting Pagefind runtime")?;
        runtime.block_on(async move {
            let mut index = PagefindIndex::new(None).context("configuring Pagefind")?;
            index
                .add_directory(site.to_string_lossy().into_owned(), None)
                .await
                .context("indexing generated pages with Pagefind")?;
            index
                .write_files(Some(site.join("pagefind").to_string_lossy().into_owned()))
                .await
                .context("writing Pagefind index")?;
            Ok(())
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Pagefind indexing thread panicked"))?
}
