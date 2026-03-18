// src/lru_cache.rs

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// LRU缓存结构体
pub struct LruCache<K, V> {
    /// 缓存的最大容量
    max_capacity: usize,
    /// 缓存项的TTL（生存时间）
    ttl: Duration,
    /// 缓存的数据存储
    cache: HashMap<K, (V, Instant)>,
    /// 用于维护LRU顺序的双向链表
    lru_list: Vec<K>,
    /// 记录缓存项的访问顺序
    access_order: HashSet<K>,
    /// 锁，用于并发安全
    lock: Arc<Mutex<()>>,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + Hash + Clone,
{
    /// 创建一个新的LRU缓存实例
    pub fn new(max_capacity: usize, ttl: Duration) -> Self {
        LruCache {
            max_capacity,
            ttl,
            cache: HashMap::new(),
            lru_list: Vec::new(),
            access_order: HashSet::new(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// 获取缓存中的值
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let _lock = self.lock.lock().unwrap();

        if let Some((value, timestamp)) = self.cache.get(key) {
            // 检查TTL是否过期
            if timestamp.elapsed() > self.ttl {
                self.remove(key);
                return None;
            }

            // 更新访问顺序
            self.access_order.insert(key.clone());
            Some(value)
        } else {
            None
        }
    }

    /// 插入或更新缓存中的值
    pub fn put(&mut self, key: K, value: V) {
        let _lock = self.lock.lock().unwrap();

        // 如果键已存在，先移除旧的
        if let Some(old_value) = self.cache.remove(&key) {
            // 移除旧的访问顺序
            self.access_order.remove(&key);
        }

        // 添加新的键值对
        self.cache.insert(key.clone(), (value, Instant::now()));
        self.lru_list.push(key);
        self.access_order.insert(key);

        // 如果超过容量，执行LRU淘汰策略
        while self.lru_list.len() > self.max_capacity {
            self.evict();
        }
    }

    /// 移除指定的键
    pub fn remove(&mut self, key: &K) {
        let _lock = self.lock.lock().unwrap();

        if let Some((_, _)) = self.cache.remove(key) {
            self.lru_list.retain(|k| k != key);
            self.access_order.remove(key);
        }
    }

    /// 清空缓存
    pub fn clear(&mut self) {
        let _lock = self.lock.lock().unwrap();

        self.cache.clear();
        self.lru_list.clear();
        self.access_order.clear();
    }

    /// 获取缓存中的所有键
    pub fn keys(&self) -> Vec<&K> {
        let _lock = self.lock.lock().unwrap();

        self.cache.keys().collect()
    }

    /// 获取缓存中的所有值
    pub fn values(&self) -> Vec<&V> {
        let _lock = self.lock.lock().unwrap();

        self.cache.values().collect()
    }

    /// 获取当前缓存中的条目数
    pub fn len(&self) -> usize {
        let _lock = self.lock.lock().unwrap();

        self.cache.len()
    }

    /// 检查缓存是否为空
    pub fn is_empty(&self) -> bool {
        let _lock = self.lock.lock().unwrap();

        self.cache.is_empty()
    }

    /// 执行LRU淘汰策略
    fn evict(&mut self) {
        let _lock = self.lock.lock().unwrap();

        // 找到最久未使用的键
        let mut min_access_time = None;
        let mut evict_key = None;

        for key in &self.lru_list {
            if let Some((_, timestamp)) = self.cache.get(key) {
                if min_access_time.map_or(true, |t| *timestamp < t) {
                    min_access_time = Some(*timestamp);
                    evict_key = Some(key.clone());
                }
            }
        }

        if let Some(evict_key) = evict_key {
            self.remove(&evict_key);
        }
    }
}

/// LRU缓存的单元测试
#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_lru_cache() {
        let mut cache = LruCache::new(2, Duration::from_secs(10));

        // 插入三个键值对，应该只保留最近使用的两个
        cache.put(1, "one");
        cache.put(2, "two");
        cache.put(3, "three");

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&1).is_none());
        assert_eq!(cache.get(&2), Some("two"));
        assert_eq!(cache.get(&3), Some("three"));

        // 再次访问2，应该更新其访问顺序
        cache.get(&2);
        cache.put(4, "four");

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&3).is_none());
        assert_eq!(cache.get(&2), Some("two"));
        assert_eq!(cache.get(&4), Some("four"));

        // 等待TTL过期，然后检查是否被自动移除
        thread::sleep(Duration::from_secs(11));
        cache.get(&2);
        assert!(cache.get(&2).is_none());
    }

    #[test]
    fn test_concurrent_access() {
        let cache = Arc::new(Mutex::new(LruCache::new(2, Duration::from_secs(10))));
        let mut handles = vec![];

        for i in 0..4 {
            let cache = Arc::clone(&cache);
            let handle = thread::spawn(move || {
                let mut cache = cache.lock().unwrap();
                cache.put(i, i);
                cache.get(&i);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let cache = cache.lock().unwrap();
        assert_eq!(cache.len(), 2);
    }
}

/// LRU缓存的性能基准测试
#[cfg(test)]
mod benchmarks {
    use super::*;
    use std::time::{Duration, Instant};

    #[bench]
    fn bench_lru_cache_insert(b: &mut bencher::Bencher) {
        let mut cache = LruCache::new(1000, Duration::from_secs(60));

        b.iter(|| {
            for i in 0..1000 {
                cache.put(i, i);
            }
        });
    }

    #[bench]
    fn bench_lru_cache_get(b: &mut bencher::Bencher) {
        let mut cache = LruCache::new(1000, Duration::from_secs(60));
        for i in 0..1000 {
            cache.put(i, i);
        }

        b.iter(|| {
            for i in 0..1000 {
                cache.get(&i);
            }
        });
    }
}