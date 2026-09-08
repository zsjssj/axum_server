use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

const DEFAULT_PAGE: i64 = 1;
const DEFAULT_PAGE_SIZE: i64 = 10;
const MAX_PAGE_SIZE: i64 = 500;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Pagination {
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_page_size")]
    page_size: i64,
}

impl Pagination {
    pub fn page(&self) -> i64 {
        self.page.max(DEFAULT_PAGE)
    }

    pub fn page_size(&self) -> i64 {
        self.page_size.clamp(1, MAX_PAGE_SIZE)
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedResponse<T> {
    pub data: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub total_pages: i64,
}

impl<T> PaginatedResponse<T> {
    pub fn new(data: Vec<T>, total: i64, pagination: &Pagination) -> Self {
        let page = pagination.page();
        let page_size = pagination.page_size();

        Self {
            data,
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
    fn normalizes_invalid_values() {
        let pagination = Pagination {
            page: 0,
            page_size: 1_000,
        };

        assert_eq!(pagination.page(), 1);
        assert_eq!(pagination.page_size(), 100);
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
