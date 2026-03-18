// tests/server_test.rs
use hyper::{Body, Request, Response, client::HttpConnector};
use hyper::client::Client;
use tokio::runtime::Runtime;
use log::{info, error};
use env_logger;
use std::time::Duration;

mod server;

#[tokio::test]
async fn test_root_route() {
    env_logger::init();

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
        info!("Starting test server on http://{}", addr);

        if let Err(e) = server.await {
            error!("Test server error: {}", e);
        }
    });

    // Wait for the server to start
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Create a client
    let client = Client::new();

    // Send a request to the root route
    let res = client.get("http://127.0.0.1:3000/".parse().unwrap()).await.unwrap();

    // Check the response
    assert_eq!(res.status(), 200);
    let body = hyper::body::to_bytes(res.into_body()).await.unwrap();
    assert_eq!(body, b"Hello, world!");

    // Shutdown the server
    server_handle.abort();
}

#[tokio::test]
async fn test_users_route() {
    env_logger::init();

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
        info!("Starting test server on http://{}", addr);

        if let Err(e) = server.await {
            error!("Test server error: {}", e);
        }
    });

    // Wait for the server to start
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Create a client
    let client = Client::new();

    // Send a request to the users route
    let res = client.get("http://127.0.0.1:3000/users".parse().unwrap()).await.unwrap();

    // Check the response
    assert_eq!(res.status(), 200);
    let body = hyper::body::to_bytes(res.into_body()).await.unwrap();
    assert_eq!(body, b"Users endpoint");

    // Shutdown the server
    server_handle.abort();
}
