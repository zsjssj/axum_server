use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

const DEFAULT_PAGE: i64 = 1;
const DEFAULT_PAGE_SIZE: i64 = 10;

#[derive(Debug, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    #[serde(default = "default_page")]
    #[validate(range(min = 1, message = "页码必须大于等于 1"))]
    #[param(minimum = 1, default = 1)]
    page: i64,
    #[serde(default = "default_page_size")]
    #[validate(range(min = 1, max = 100, message = "每页数量必须在 1 到 100 之间"))]
    #[param(minimum = 1, maximum = 100, default = 10)]
    page_size: i64,
}

impl Pagination {
    pub fn page(&self) -> i64 {
        self.page
    }

    pub fn page_size(&self) -> i64 {
        self.page_size
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub total_pages: i64,
}

impl<T> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, total: i64, pagination: &Pagination) -> Self {
        let page = pagination.page();
        let page_size = pagination.page_size();

        Self {
            items,
            total,
            page,
            page_size,
            total_pages: (total + page_size - 1) / page_size,
        }
    }
}

fn default_page() -> i64 {
    DEFAULT_PAGE
}

fn default_page_size() -> i64 {
    DEFAULT_PAGE_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_values() {
        let pagination = Pagination {
            page: 0,
            page_size: 1_000,
        };

        let errors = pagination.validate().expect_err("非法分页参数应校验失败");

        assert!(errors.field_errors().contains_key("page"));
        assert!(errors.field_errors().contains_key("page_size"));
    }

    #[test]
    fn calculates_total_pages_without_floating_point() {
        let pagination = Pagination {
            page: 1,
            page_size: 10,
        };
        let response = PaginatedResponse::new(Vec::<()>::new(), 21, &pagination);

        assert_eq!(response.total_pages, 3);
    }
}
