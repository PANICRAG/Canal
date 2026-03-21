// tests/race_condition_fixed.rs
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Fixed LRU Cache implementation without race conditions
pub struct LruCache<K, V> {
    max_capacity: usize,
    ttl: Duration,
    cache: HashMap<K, (V, Instant)>,
    lru_list: Vec<K>,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
{
    pub fn new(max_capacity: usize, ttl: Duration) -> Self {
        LruCache {
            max_capacity,
            ttl,
            cache: HashMap::new(),
            lru_list: Vec::new(),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        if let Some((value, timestamp)) = self.cache.get(key) {
            if timestamp.elapsed() > self.ttl {
                // Remove expired entry
                self.cache.remove(key);
                self.lru_list.retain(|k| k != key);
                return None;
            }
            Some(value.clone())
        } else {
            None
        }
    }

    pub fn put(&mut self, key: K, value: V) {
        // Remove existing key if present
        if self.cache.contains_key(&key) {
            self.cache.remove(&key);
            self.lru_list.retain(|k| k != &key);
        }

        self.cache.insert(key.clone(), (value, Instant::now()));
        self.lru_list.push(key);

        // Evict if over capacity
        while self.lru_list.len() > self.max_capacity {
            if let Some(evict_key) = self.lru_list.first().cloned() {
                self.cache.remove(&evict_key);
                self.lru_list.remove(0);
            }
        }
    }

    pub fn remove(&mut self, key: &K) {
        self.cache.remove(key);
        self.lru_list.retain(|k| k != key);
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }
}

fn main() {
    println!("Running fixed race condition test...");
    
    // Create a shared LRU cache with small capacity
    let cache = Arc::new(Mutex::new(LruCache::new(5, Duration::from_secs(60))));
    
    let mut handles = vec![];
    
    // Spawn multiple threads that will simultaneously read and write
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
                thread::yield_now();
            }
        });
        handles.push(handle);
    }
    
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
    
    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Verify cache integrity
    let cache = cache.lock().unwrap();
    let len = cache.len();
    println!("Final cache size: {}", len);
    
    if len <= 5 {
        println!("✅ Test passed - cache size within limits");
    } else {
        println!("❌ Test failed - cache size {} exceeds capacity 5", len);
    }
}