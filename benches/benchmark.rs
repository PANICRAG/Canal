// benches/benchmark.rs
use hyper::{Body, Request, client::HttpConnector};
use hyper::client::Client;
use tokio::runtime::Runtime;
use std::time::{Duration, Instant};
use criterion::{black_box, Criterion};

mod server;

fn bench_server(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let handle = rt.handle().clone();

    // Start the server in a separate thread
    let server_handle = handle.spawn(async {
        let addr = "127.0.0.1:3000".parse().unwrap();
        let make_svc = server::make_service_fn(|_conn| {
            async move {
                Ok::<_, hyper::Error>(server::service_fn(|req| {
                    server::handle_request(req)
                }))
            }
        });

        let server = hyper::Server::bind(&addr).serve(make_svc);
        println!("Starting benchmark server on http://{}", addr);

        if let Err(e) = server.await {
            eprintln!("Benchmark server error: {}", e);
        }
    });

    // Wait for the server to start
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Create a client
    let client = Client::new();

    // Benchmark the root route
    c.bench_function("root_route", |b| {
        b.iter(|| {
            let req = Request::get("http://127.0.0.1:3000/").body(Body::empty()).unwrap();
            let res = client.request(black_box(req)).unwrap();
            assert_eq!(res.status(), 200);
        })
    });

    // Benchmark the users route
    c.bench_function("users_route", |b| {
        b.iter(|| {
            let req = Request::get("http://127.0.0.1:3000/users").body(Body::empty()).unwrap();
            let res = client.request(black_box(req)).unwrap();
            assert_eq!(res.status(), 200);
        })
    });

    // Shutdown the server
    server_handle.abort();
}

criterion_group!(benches, bench_server);
criterion_main!(benches);
