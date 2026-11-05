use thiserror::Error;

#[derive(Debug, Error)]
pub enum OfdError {
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP 错误: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("包内条目不存在: {0}")]
    EntryNotFound(String),

    #[error("非法包内路径: {0}")]
    InvalidPath(String),

    #[error("{file} XML 解析失败: {source}")]
    Xml {
        file: String,
        #[source]
        source: roxmltree::Error,
    },

    #[error("不是 UTF-8 编码的 XML: {0}")]
    NonUtf8(String),

    #[error("缺少必要元素: {0}")]
    MissingElement(String),

    #[error("缺少必要属性: {0}")]
    MissingAttribute(String),

    #[error("非法数值 {what}: {value}")]
    InvalidValue { what: String, value: String },

    #[error("超出安全限制: {0}")]
    LimitExceeded(&'static str),

    #[error("暂不支持: {0}")]
    Unsupported(String),

    #[error("渲染失败: {0}")]
    Render(String),
}

pub type Result<T> = std::result::Result<T, OfdError>;
