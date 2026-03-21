//! Performance benchmarks for the Filesystem Service
//!
//! Run with: cargo bench -p gateway-core

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use gateway_core::filesystem::{
    DirectoryConfig, DirectoryMode, FilesystemConfig, FilesystemService,
};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn canonical_temp_path(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().canonicalize().unwrap()
}

fn create_test_service(temp_dir: &TempDir) -> Arc<FilesystemService> {
    let canonical_path = canonical_temp_path(temp_dir);
    let config = FilesystemConfig {
        enabled: true,
        allowed_directories: vec![DirectoryConfig {
            path: canonical_path.to_string_lossy().to_string(),
            mode: DirectoryMode::ReadWrite,
            description: Some("Benchmark directory".to_string()),
            docker_mount_path: None,
        }],
        blocked_patterns: vec![".env".to_string()],
        max_read_bytes: 100 * 1024 * 1024, // 100MB for benchmarks
        max_write_bytes: 100 * 1024 * 1024,
        follow_symlinks: true,
        default_encoding: "utf-8".to_string(),
    };
    Arc::new(FilesystemService::new(config))
}

fn bench_file_read(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let service = create_test_service(&temp_dir);
    let base = canonical_temp_path(&temp_dir);

    // Create test files of different sizes
    let sizes = [1024, 10 * 1024, 100 * 1024, 1024 * 1024]; // 1KB, 10KB, 100KB, 1MB

    for size in sizes {
        let test_file = base.join(format!("test_{}.txt", size));
        let content = "x".repeat(size);
        std::fs::write(&test_file, &content).unwrap();
    }

    let mut group = c.benchmark_group("file_read");

    for size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        let test_file = base.join(format!("test_{}.txt", size));
        let path_str = test_file.to_string_lossy().to_string();
        let svc = service.clone();

        group.bench_with_input(BenchmarkId::from_parameter(size), &path_str, |b, path| {
            let svc = svc.clone();
            let path = path.clone();
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let path = path.clone();
                async move { black_box(svc.read_file(&path).await.unwrap()) }
            })
        });
    }

    group.finish();
}

fn bench_file_write(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let service = create_test_service(&temp_dir);
    let base = canonical_temp_path(&temp_dir);

    let sizes = [1024, 10 * 1024, 100 * 1024]; // 1KB, 10KB, 100KB

    let mut group = c.benchmark_group("file_write");

    for size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        let content = Arc::new("x".repeat(size));
        let svc = service.clone();
        let base_clone = base.clone();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
            let svc = svc.clone();
            let content = content.clone();
            let base = base_clone.clone();
            let counter = std::sync::atomic::AtomicU64::new(0);

            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let content = content.clone();
                let base = base.clone();
                let cnt = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    let test_file = base.join(format!("write_{}_{}.txt", sz, cnt));
                    let path_str = test_file.to_string_lossy().to_string();
                    black_box(
                        svc.write_file(&path_str, &content, false, true)
                            .await
                            .unwrap(),
                    )
                }
            })
        });
    }

    group.finish();
}

fn bench_directory_list(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let service = create_test_service(&temp_dir);
    let base = canonical_temp_path(&temp_dir);

    // Create directories with different number of files
    let file_counts = [10, 100, 500];

    for count in file_counts {
        let subdir = base.join(format!("dir_{}", count));
        std::fs::create_dir(&subdir).unwrap();
        for i in 0..count {
            std::fs::write(subdir.join(format!("file_{}.txt", i)), "content").unwrap();
        }
    }

    let mut group = c.benchmark_group("directory_list");

    for count in file_counts {
        let subdir = base.join(format!("dir_{}", count));
        let path_str = subdir.to_string_lossy().to_string();
        let svc = service.clone();

        group.bench_with_input(BenchmarkId::from_parameter(count), &path_str, |b, path| {
            let svc = svc.clone();
            let path = path.clone();
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let path = path.clone();
                async move { black_box(svc.list_directory(&path, false, false).await.unwrap()) }
            })
        });
    }

    group.finish();
}

fn bench_search(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let service = create_test_service(&temp_dir);
    let base = canonical_temp_path(&temp_dir);

    // Create test files with searchable content
    let search_dir = base.join("search_test");
    std::fs::create_dir(&search_dir).unwrap();

    // Create 50 files with varying content
    for i in 0..50 {
        let content = format!(
            "// File {}\nfn function_{}() {{\n    println!(\"Hello {}\");\n}}\n",
            i, i, i
        );
        std::fs::write(search_dir.join(format!("file_{}.rs", i)), content).unwrap();
    }

    let path_str = search_dir.to_string_lossy().to_string();

    let mut group = c.benchmark_group("search");

    // Benchmark different patterns
    {
        let svc = service.clone();
        let path = path_str.clone();
        group.bench_function("simple_literal", |b| {
            let svc = svc.clone();
            let path = path.clone();
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let path = path.clone();
                async move { black_box(svc.search(&path, "println", None, 100).await.unwrap()) }
            })
        });
    }

    {
        let svc = service.clone();
        let path = path_str.clone();
        group.bench_function("pattern_match", |b| {
            let svc = svc.clone();
            let path = path.clone();
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let path = path.clone();
                async move {
                    black_box(
                        svc.search(&path, "function_[0-9]+", None, 100)
                            .await
                            .unwrap(),
                    )
                }
            })
        });
    }

    {
        let svc = service.clone();
        let path = path_str.clone();
        group.bench_function("with_file_filter", |b| {
            let svc = svc.clone();
            let path = path.clone();
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let path = path.clone();
                async move {
                    black_box(
                        svc.search(&path, "println", Some("*.rs"), 100)
                            .await
                            .unwrap(),
                    )
                }
            })
        });
    }

    group.finish();
}

fn bench_permission_checks(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let service = create_test_service(&temp_dir);
    let base = canonical_temp_path(&temp_dir);

    let test_path = base.join("test.txt").to_string_lossy().to_string();
    let blocked_path = base.join(".env").to_string_lossy().to_string();
    let outside_path = "/etc/passwd".to_string();

    let mut group = c.benchmark_group("permission_checks");

    {
        let path = test_path.clone();
        let svc = service.clone();
        group.bench_function("can_read_allowed", |b| {
            b.iter(|| black_box(svc.can_read(&path)))
        });
    }

    {
        let path = blocked_path.clone();
        let svc = service.clone();
        group.bench_function("can_read_blocked", |b| {
            b.iter(|| black_box(svc.can_read(&path)))
        });
    }

    {
        let path = outside_path.clone();
        let svc = service.clone();
        group.bench_function("can_read_outside", |b| {
            b.iter(|| black_box(svc.can_read(&path)))
        });
    }

    {
        let path = test_path.clone();
        let svc = service.clone();
        group.bench_function("can_write_allowed", |b| {
            b.iter(|| black_box(svc.can_write(&path)))
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_file_read,
    bench_file_write,
    bench_directory_list,
    bench_search,
    bench_permission_checks
);
criterion_main!(benches);
