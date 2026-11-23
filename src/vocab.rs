//! IRIs of every vocabulary term triplify emits.
//!
//! All `raw` terms are defined, with SHACL shapes, in
//! <https://ekgf.github.io/ekg-ontologies/spec/main/ekg-raw/index.html>.

macro_rules! terms {
    ($ns:literal; $($name:ident = $local:literal),* $(,)?) => {
        $(pub const $name: ::oxrdf::NamedNodeRef<'static> =
            ::oxrdf::NamedNodeRef::new_unchecked(concat!($ns, $local));)*
    };
}

pub mod raw {
    pub const NS: &str = "https://ekgf.org/ontology/raw#";
    terms! { "https://ekgf.org/ontology/raw#";
        SOURCE = "Source", FILE = "File", XLSX_FILE = "XlsxFile",
        VIEW = "View", COLUMN = "Column", ROW = "Row", VALUE = "Value",
        NULL_VALUE = "NullValue",
        HAS_VIEW = "hasView", IS_VIEW_IN = "isViewIn",
        HAS_COLUMN = "hasColumn", IS_COLUMN_IN_VIEW = "isColumnInView",
        IS_ROW_IN_VIEW = "isRowInView", VALUE_OF = "valueOf",
        FOR_COLUMN = "forColumn", LITERAL_VALUE = "literalValue",
        SOURCE_TEXT = "sourceText", COLUMN_NUMBER = "columnNumber",
        VIEW_ROW_NUMBER = "viewRowNumber", FILE_NAME = "fileName",
        CSV_FILE = "CsvFile", TSV_FILE = "TsvFile", PARQUET_FILE = "ParquetFile",
        VIEW_NUMBER = "viewNumber",
        GIT_REPOSITORY = "gitRepository", GIT_COMMIT = "gitCommit",
        GIT_PATH = "gitPath", GIT_BLOB = "gitBlob",
        GIT_MODIFIED = "gitModified",
        FORMULA = "formula", FORMULA_R1C1 = "formulaR1C1",
        COUNT = "count", NULL_COUNT = "nullCount", UNIQUE = "unique",
        TOP = "top", FREQ = "freq", INFERRED_DATATYPE = "inferredDatatype",
        CONFORMING_COUNT = "conformingCount", MIN = "min", MAX = "max",
        MEAN = "mean", STD = "std", P25 = "25pct", P50 = "50pct",
        P75 = "75pct",
        S3_OBJECT = "S3Object", S3_TAG_CLASS = "S3Tag",
        S3_USER_METADATA_ENTRY = "S3UserMetadataEntry",
        S3_BUCKET = "s3Bucket", S3_KEY = "s3Key", S3_REGION = "s3Region",
        S3_ENDPOINT = "s3Endpoint", S3_VERSION_ID = "s3VersionId",
        S3_ETAG = "s3ETag", S3_CONTENT_TYPE = "s3ContentType",
        S3_CONTENT_ENCODING = "s3ContentEncoding",
        S3_CONTENT_LANGUAGE = "s3ContentLanguage",
        S3_CONTENT_DISPOSITION = "s3ContentDisposition",
        S3_CACHE_CONTROL = "s3CacheControl",
        S3_STORAGE_CLASS = "s3StorageClass",
        S3_SERVER_SIDE_ENCRYPTION = "s3ServerSideEncryption",
        S3_KMS_KEY_ID = "s3KmsKeyId",
        S3_BUCKET_KEY_ENABLED = "s3BucketKeyEnabled",
        S3_CHECKSUM_TYPE = "s3ChecksumType",
        S3_CHECKSUM_CRC32 = "s3ChecksumCrc32",
        S3_CHECKSUM_CRC32C = "s3ChecksumCrc32c",
        S3_CHECKSUM_CRC64NVME = "s3ChecksumCrc64Nvme",
        S3_CHECKSUM_SHA1 = "s3ChecksumSha1",
        S3_CHECKSUM_SHA256 = "s3ChecksumSha256",
        S3_PARTS_COUNT = "s3PartsCount",
        S3_OBJECT_LOCK_MODE = "s3ObjectLockMode",
        S3_OBJECT_LOCK_RETAIN_UNTIL = "s3ObjectLockRetainUntil",
        S3_OBJECT_LOCK_LEGAL_HOLD = "s3ObjectLockLegalHold",
        S3_REPLICATION_STATUS = "s3ReplicationStatus",
        S3_EXPIRATION = "s3Expiration",
        S3_TAG = "s3Tag", S3_USER_METADATA = "s3UserMetadata",
        S3_ENTRY_KEY = "s3EntryKey", S3_ENTRY_VALUE = "s3EntryValue",
    }
}

pub mod rdf {
    terms! { "http://www.w3.org/1999/02/22-rdf-syntax-ns#"; TYPE = "type" }
}

pub mod rdfs {
    terms! { "http://www.w3.org/2000/01/rdf-schema#"; LABEL = "label" }
}

pub mod prov {
    terms! { "http://www.w3.org/ns/prov#";
        ENTITY = "Entity", ACTIVITY = "Activity",
        SOFTWARE_AGENT = "SoftwareAgent",
        USED = "used", WAS_GENERATED_BY = "wasGeneratedBy",
        WAS_DERIVED_FROM = "wasDerivedFrom",
        WAS_ASSOCIATED_WITH = "wasAssociatedWith",
        STARTED_AT_TIME = "startedAtTime", ENDED_AT_TIME = "endedAtTime",
        AT_LOCATION = "atLocation",
    }
}

pub mod dcat {
    terms! { "http://www.w3.org/ns/dcat#";
        DATASET = "Dataset", DISTRIBUTION = "Distribution",
        DISTRIBUTION_P = "distribution", MEDIA_TYPE = "mediaType",
        BYTE_SIZE = "byteSize", DOWNLOAD_URL = "downloadURL",
    }
}

pub mod dcterms {
    terms! { "http://purl.org/dc/terms/";
        MODIFIED = "modified", TITLE = "title", IDENTIFIER = "identifier",
    }
}

pub mod spdx {
    terms! { "http://spdx.org/rdf/terms#";
        CHECKSUM_CLASS = "Checksum", CHECKSUM = "checksum",
        ALGORITHM = "algorithm", CHECKSUM_VALUE = "checksumValue",
        SHA256 = "checksumAlgorithm_sha256",
    }
}

pub mod xsd {
    terms! { "http://www.w3.org/2001/XMLSchema#";
        STRING = "string", INTEGER = "integer", DOUBLE = "double",
        BOOLEAN = "boolean", DECIMAL = "decimal", DATE = "date", TIME = "time", DATE_TIME = "dateTime", DURATION = "duration",
        NON_NEGATIVE_INTEGER = "nonNegativeInteger",
        HEX_BINARY = "hexBinary",
    }
}
