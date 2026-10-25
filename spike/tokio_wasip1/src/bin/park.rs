//! Probe: what does a stock tokio current_thread runtime do on wasm32-wasip1
//! when it parks for a timer?
use std::time::Instant;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let t = Instant::now();
    rt.block_on(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });
    println!("slept, elapsed = {:?}", t.elapsed());
}
