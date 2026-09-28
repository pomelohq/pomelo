//! Which engine a database or shared service runs, told from its image name, and what the panel can do with it.

use pom_config::SharedServiceDef;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    Mysql,
    Mariadb,
    Redis,
    Mongodb,
    Sqlite,
    Elasticsearch,
    Opensearch,
    Minio,
    Rabbitmq,
    Kafka,
    /// An image this list does not know.
    Other,
}

/// Image-name fragments in match order: `mariadb` images often mention mysql, so it is tried first.
const IMAGE_FRAGMENTS: [(&str, Engine); 12] = [
    ("postgis", Engine::Postgres),
    ("postgres", Engine::Postgres),
    ("mariadb", Engine::Mariadb),
    ("mysql", Engine::Mysql),
    ("redis", Engine::Redis),
    ("mongo", Engine::Mongodb),
    ("opensearch", Engine::Opensearch),
    ("elasticsearch", Engine::Elasticsearch),
    ("minio", Engine::Minio),
    ("rabbitmq", Engine::Rabbitmq),
    ("kafka", Engine::Kafka),
    ("sqlite", Engine::Sqlite),
];

impl Engine {
    /// The engine an image runs (`postgres:16`, `minio/minio`, `bitnami/redis:7`), by its repository name.
    pub fn from_image(image: &str) -> Option<Engine> {
        let repository = image.rsplit('/').next().unwrap_or(image);
        let repository = repository.split([':', '@']).next().unwrap_or(repository);
        let repository = repository.to_ascii_lowercase();
        IMAGE_FRAGMENTS
            .iter()
            .find(|(fragment, _)| repository.contains(fragment))
            .map(|(_, engine)| *engine)
    }

    /// A shared service's engine: its image, else its `type:`, else its name.
    pub fn of_service(name: &str, service: &SharedServiceDef) -> Engine {
        Engine::from_image(&service.image)
            .or_else(|| Engine::from_image(&service.kind))
            .or_else(|| Engine::from_image(name))
            .unwrap_or(Engine::Other)
    }

    pub fn title(self) -> &'static str {
        match self {
            Engine::Postgres => "PostgreSQL",
            Engine::Mysql => "MySQL",
            Engine::Mariadb => "MariaDB",
            Engine::Redis => "Redis",
            Engine::Mongodb => "MongoDB",
            Engine::Sqlite => "SQLite",
            Engine::Elasticsearch => "Elasticsearch",
            Engine::Opensearch => "OpenSearch",
            Engine::Minio => "MinIO",
            Engine::Rabbitmq => "RabbitMQ",
            Engine::Kafka => "Kafka",
            Engine::Other => "Service",
        }
    }

    /// Whether the panel can open it: list its tables, keys or objects.
    pub fn browsable(self) -> bool {
        matches!(self, Engine::Postgres | Engine::Redis | Engine::Minio)
    }

    /// Whether it holds tables to query with SQL.
    pub fn is_sql(self) -> bool {
        self == Engine::Postgres
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_map_to_their_engine() {
        let cases = [
            ("postgres:16", Some(Engine::Postgres)),
            ("postgis/postgis:15-3.4", Some(Engine::Postgres)),
            ("mysql:8", Some(Engine::Mysql)),
            ("mariadb:11", Some(Engine::Mariadb)),
            ("bitnami/redis:7.2", Some(Engine::Redis)),
            ("redis:7-alpine", Some(Engine::Redis)),
            ("mongo:7", Some(Engine::Mongodb)),
            (
                "opensearchproject/opensearch:2.11.0",
                Some(Engine::Opensearch),
            ),
            (
                "docker.elastic.co/elasticsearch/elasticsearch:8.12.0",
                Some(Engine::Elasticsearch),
            ),
            ("minio/minio", Some(Engine::Minio)),
            ("rabbitmq:3-management", Some(Engine::Rabbitmq)),
            ("confluentinc/cp-kafka:7.5.0", Some(Engine::Kafka)),
            ("public.ecr.aws/zinclabs/zincsearch:latest", None),
            (
                "registry.example.com:5000/team/mysql:8",
                Some(Engine::Mysql),
            ),
        ];
        for (image, engine) in cases {
            assert_eq!(Engine::from_image(image), engine, "{image}");
        }
    }

    #[test]
    fn a_service_without_an_image_goes_by_its_type_then_its_name() {
        let typed = SharedServiceDef {
            kind: "redis".into(),
            ..SharedServiceDef::default()
        };
        assert_eq!(Engine::of_service("cache", &typed), Engine::Redis);
        assert_eq!(
            Engine::of_service("minio", &SharedServiceDef::default()),
            Engine::Minio
        );
        assert_eq!(
            Engine::of_service("mailpit", &SharedServiceDef::default()),
            Engine::Other
        );
        assert!(Engine::Minio.browsable() && !Engine::Mysql.browsable());
    }
}
