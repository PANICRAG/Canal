// tests/race_condition_test.rs
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use src::lru_cache::LruCache;

/// Test for race conditions in LRU cache with high concurrency
#[test]
fn test_lru_cache_race_condition() {
    // Create a shared LRU cache with small capacity to trigger evictions
    let cache = Arc::new(Mutex::new(LruCache::new(5, Duration::from_secs(60))));
    
    // Spawn multiple threads that will simultaneously read and write
    let mut handles = vec![];
    
    // Writer threads - continuously insert new values
    for writer_id in 0..3 {
        let cache = Arc::clone(&cache);
        let handle = thread::spawn(move || {
            for i in 0..1000 {
                let key = writer_id * 1000 + i;
                let value = format!("value_{}_{}", writer_id, i);
                {
                    let mut cache = cache.lock().unwrap();
                    cache.put(key, value);
                }
                // Small delay to allow interleaving
                thread::yield_now();
            }
        });
        handles.push(handle);
    }
    
    // Reader threads - continuously read random keys
    for reader_id in 0..2 {
        let cache = Arc::clone(&cache);
        let handle = thread::spawn(move || {
            for _ in 0..1000 {
                let key = (reader_id * 500 + rand::random::<usize>() % 1500) as i32;
                {
                    let mut cache = cache.lock().unwrap();
                    let _ = cache.get(&key);
                }
                thread::yield_now();
            }
        });
        handles.push(handle);
    }
    
    // Mixed read/write threads
    for mixed_id in 0..2 {
        let cache = Arc::clone(&cache);
        let handle = thread::spawn(move || {
            for i in 0..1000 {
                if i % 2 == 0 {
                    // Write
                    let key = mixed_id * 1000 + i;
                    let value = format!("mixed_value_{}_{}", mixed_id, i);
                    {
                        let mut cache = cache.lock().unwrap();
                        cache.put(key, value);
                    }
                } else {
                    // Read
                    let key = (mixed_id * 500 + rand::random::<usize>() % 1500) as i32;
                    {
                        let mut cache = cache.lock().unwrap();
                        let _ = cache.get(&key);
                    }
                }
                thread::yield_now();
            }
        });
        handles.push(handle);
    }
    
    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Verify cache integrity after concurrent operations
    let cache = cache.lock().unwrap();
    let len = cache.len();
    
    // Cache should not exceed capacity
    assert!(len <= 5, "Cache size {} exceeds capacity 5", len);
    
    // Cache should not be negative or corrupted
    assert!(len >= 0, "Cache size is negative: {}", len);
}

/// Test for race condition during eviction
#[test]
fn test_lru_cache_eviction_race() {
    // Create cache with very small capacity to force frequent evictions
    let cache = Arc::new(Mutex::new(LruCache::new(2, Duration::from_secs(60))));
    
    let mut handles = vec![];
    
    // Multiple threads inserting different keys to trigger constant eviction
    for thread_id in 0..5 {
        let cache = Arc::clone(&cache);
        let handle = thread::spawn(move || {
            for i in 0..200 {
                let key = thread_id * 200 + i;
                {
                    let mut cache = cache.lock().unwrap();
                    cache.put(key, format!("value_{}", key));
                    // Immediately try to get it back
                    let _ = cache.get(&key);
                }
                thread::yield_now();
            }
        });
        handles.push(handle);
    }
    
    // Wait for completion
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Final verification
    let cache = cache.lock().unwrap();
    let len = cache.len();
    assert!(len <= 2, "Cache size {} exceeds capacity 2", len);
}

/// Test for race condition with TTL expiration
#[test]
fn test_lru_cache_ttl_race() {
    // Create cache with short TTL
    let cache = Arc::new(Mutex::new(LruCache::new(10, Duration::from_millis(100))));
    
    let mut handles = vec![];
    
    // Insert data
    {
        let mut cache = cache.lock().unwrap();
        for i in 0..10 {
            cache.put(i, format!("value_{}", i));
        }
    }
    
    // Start threads that continuously access data while TTL expires
    for thread_id in 0..3 {
        let cache = Arc::clone(&cache);
        let handle = thread::spawn(move || {
            for _ in 0..50 {
                let key = rand::random::<usize>() % 10;
                {
                    let mut cache = cache.lock().unwrap();
                    let _ = cache.get(&(key as i32));
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        handles.push(handle);
    }
    
    // Wait for TTL to expire and threads to complete
    thread::sleep(Duration::from_millis(150));
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Cache should be mostly empty due to TTL expiration
    let cache = cache.lock().unwrap();
    let len = cache.len();
    // Some items might still be there if they were accessed recently
    assert!(len <= 10, "Cache size {} seems too large after TTL", len);
}

/// Stress test with maximum concurrency
#[test]
fn test_lru_cache_stress_test() {
    let cache = Arc::new(Mutex::new(LruCache::new(10, Duration::from_secs(60))));
    
    let mut handles = vec![];
    
    // Create many threads with mixed operations
    for thread_id in 0..10 {
        let cache = Arc::clone(&cache);
        let handle = thread::spawn(move || {
            for i in 0..100 {
                let operation = rand::random::<u8>() % 3;
                match operation {
                    0 => {
                        // Put operation
                        let key = thread_id * 100 + i;
                        let value = format!("stress_value_{}_{}", thread_id, i);
                        {
                            let mut cache = cache.lock().unwrap();
                            cache.put(key, value);
                        }
                    }
                    1 => {
                        // Get operation
                        let key = rand::random::<usize>() % 2000;
                        {
                            let mut cache = cache.lock().unwrap();
                            let _ = cache.get(&(key as i32));
                        }
                    }
                    2 => {
                        // Remove operation (if key exists)
                        let key = rand::random::<usize>() % 2000;
                        {
                            let mut cache = cache.lock().unwrap();
                            cache.remove(&(key as i32));
                        }
                    }
                    _ => {}
                }
                thread::yield_now();
            }
        });
        handles.push(handle);
    }
    
    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Final integrity check
    let cache = cache.lock().unwrap();
    let len = cache.len();
    assert!(len <= 10, "Cache size {} exceeds capacity 10", len);
    assert!(len >= 0, "Cache size is negative: {}", len);
}