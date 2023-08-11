use error_stack::ResultExt;
use masking::StrongSecret;
use redis::CacheStore;
pub mod config;
pub mod diesel;
pub mod redis;

pub use crate::diesel::store::DatabaseStore;

#[allow(dead_code)]
pub struct RouterStore<T: DatabaseStore> {
    db_store: T,
    cache_store: CacheStore,
    master_encryption_key: StrongSecret<Vec<u8>>,
}

impl<T: DatabaseStore> RouterStore<T> {
    pub async fn new(
        db_conf: T::Config,
        cache_conf: &redis_interface::RedisSettings,
        encryption_key: StrongSecret<Vec<u8>>,
        cache_error_signal: tokio::sync::oneshot::Sender<()>,
        inmemory_cache_stream: &str,
    ) -> Self {
        // TODO: create an error enum and return proper error here
        let db_store = T::new(db_conf, false).await;
        #[allow(clippy::expect_used)]
        let cache_store = CacheStore::new(cache_conf)
            .await
            .expect("Failed to create cache store");
        cache_store.set_error_callback(cache_error_signal);
        #[allow(clippy::expect_used)]
        cache_store
            .subscribe_to_channel(inmemory_cache_stream)
            .await
            .expect("Failed to subscribe to inmemory cache stream");
        Self {
            db_store,
            cache_store,
            master_encryption_key: encryption_key,
        }
    }
    pub async fn test_store(
        db_conf: T::Config,
        cache_conf: &redis_interface::RedisSettings,
        encryption_key: StrongSecret<Vec<u8>>,
    ) -> Self {
        // TODO: create an error enum and return proper error here
        let db_store = T::new(db_conf, true).await;
        #[allow(clippy::expect_used)]
        let cache_store = CacheStore::new(cache_conf)
            .await
            .expect("Failed to create cache store");
        Self {
            db_store,
            cache_store,
            master_encryption_key: encryption_key,
        }
    }
}

pub struct KVRouterStore<T: DatabaseStore> {
    router_store: RouterStore<T>,
    drainer_stream_name: String,
    drainer_num_partitions: u8,
}

impl<T: DatabaseStore> KVRouterStore<T> {
    pub fn from_store(
        store: RouterStore<T>,
        drainer_stream_name: String,
        drainer_num_partitions: u8,
    ) -> Self {
        Self {
            router_store: store,
            drainer_stream_name,
            drainer_num_partitions,
        }
    }

    pub fn get_drainer_stream_name(&self, shard_key: &str) -> String {
        format!("{{{}}}_{}", shard_key, self.drainer_stream_name)
    }

    #[allow(dead_code)]
    async fn push_to_drainer_stream<R>(
        &self,
        redis_entry: diesel_models::kv::TypedSql,
        partition_key: redis::kv_store::PartitionKey<'_>,
    ) -> error_stack::Result<(), redis_interface::errors::RedisError>
    where
        R: crate::redis::kv_store::KvStorePartition,
    {
        let shard_key = R::shard_key(partition_key, self.drainer_num_partitions);
        let stream_name = self.get_drainer_stream_name(&shard_key);
        self.router_store
            .cache_store
            .redis_conn
            .stream_append_entry(
                &stream_name,
                &redis_interface::RedisEntryId::AutoGeneratedID,
                redis_entry
                    .to_field_value_pairs()
                    .change_context(redis_interface::errors::RedisError::JsonSerializationFailed)?,
            )
            .await
            .change_context(redis_interface::errors::RedisError::StreamAppendFailed)
    }
}
