//! Explicit disk benchmark; never runs as part of the normal test suite.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::fs::File;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::task::JoinSet;

const MIB: usize = 1024 * 1024;
const TOTAL_BYTES: usize = 64 * MIB;
const CAPACITIES: [usize; 4] = [0, 256 * 1024, 512 * 1024, MIB];

// Counts successful writes accepted by Tokio's file writer, not OS syscalls.
struct CountedFile {
    file: File,
    writes: u64,
}

impl AsyncWrite for CountedFile {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.file).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Ok(n)) if n > 0) {
            self.writes += 1;
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.file).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.file).poll_shutdown(cx)
    }
}

async fn write_chunks(writer: &mut (impl AsyncWrite + Unpin), chunk: &[u8], bytes: usize) {
    for _ in 0..bytes / chunk.len() {
        writer.write_all(chunk).await.unwrap();
    }
    writer.flush().await.unwrap();
}

async fn measure(capacity: usize, chunk_size: usize, workers: usize) -> (f64, u64) {
    let root = tempfile::tempdir().unwrap();
    let chunk: Arc<Vec<u8>> = Arc::new((0..chunk_size).map(|i| (i % 251) as u8).collect());
    let bytes_per_worker = TOTAL_BYTES / workers;
    let mut files = Vec::new();
    for index in 0..workers {
        let path = root.path().join(format!("worker-{index}.part"));
        files.push((path.clone(), File::create(path).await.unwrap()));
    }
    let start = Instant::now();
    let mut tasks = JoinSet::new();
    for (path, file) in files {
        let chunk = chunk.clone();
        tasks.spawn(async move {
            let mut file = CountedFile { file, writes: 0 };
            if capacity == 0 {
                write_chunks(&mut file, &chunk, bytes_per_worker).await;
            } else {
                let mut writer = BufWriter::with_capacity(capacity, file);
                write_chunks(&mut writer, &chunk, bytes_per_worker).await;
                file = writer.into_inner();
            }
            file.file.sync_all().await.unwrap();
            (path, file.writes)
        });
    }
    let mut outputs = Vec::new();
    while let Some(result) = tasks.join_next().await {
        outputs.push(result.unwrap());
    }
    let elapsed = start.elapsed().as_secs_f64();
    let writes = outputs.iter().map(|(_, writes)| writes).sum();
    // Validate every byte outside the timed region.
    for (path, _) in outputs {
        let bytes = tokio::fs::read(path).await.unwrap();
        assert_eq!(bytes.len(), bytes_per_worker);
        for actual in bytes.chunks(chunk_size) {
            assert_eq!(actual, chunk.as_slice());
        }
    }
    (TOTAL_BYTES as f64 / MIB as f64 / elapsed, writes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Writes and verifies 6 GiB of temporary files; run explicitly for buffer tuning"]
async fn segment_write_buffer_benchmark() {
    println!("capacity_KiB,chunk_KiB,workers,median_MiB_s,writer_calls,buffer_KiB");
    let mut log_throughputs = [0.0f64; 4];
    let mut cases = 0;
    for workers in [1, 8] {
        for chunk_size in [16 * 1024, 64 * 1024, 256 * 1024, MIB] {
            let mut samples: [Vec<(f64, u64)>; 4] = std::array::from_fn(|_| Vec::new());
            // Rotate execution order so the same capacity is not always first.
            for repetition in 0..3 {
                for offset in 0..CAPACITIES.len() {
                    let index = (repetition + offset) % CAPACITIES.len();
                    samples[index].push(measure(CAPACITIES[index], chunk_size, workers).await);
                }
            }
            for (index, values) in samples.iter_mut().enumerate() {
                values.sort_by(|a, b| a.0.total_cmp(&b.0));
                let (throughput, writes) = values[1];
                log_throughputs[index] += throughput.ln();
                println!(
                    "{},{},{},{:.2},{},{}",
                    CAPACITIES[index] / 1024,
                    chunk_size / 1024,
                    workers,
                    throughput,
                    writes,
                    CAPACITIES[index] * workers / 1024
                );
            }
            cases += 1;
        }
    }
    let geometric_means = log_throughputs.map(|sum| (sum / cases as f64).exp());
    let best = geometric_means[1..].iter().copied().fold(0.0, f64::max);
    let selected = (1..CAPACITIES.len())
        .find(|&index| geometric_means[index] >= best * 0.95)
        .unwrap();
    for (index, mean) in geometric_means.iter().enumerate() {
        println!(
            "geometric_mean capacity_KiB={} MiB_s={mean:.2}",
            CAPACITIES[index] / 1024
        );
    }
    println!("selected_capacity_KiB={}", CAPACITIES[selected] / 1024);
}
