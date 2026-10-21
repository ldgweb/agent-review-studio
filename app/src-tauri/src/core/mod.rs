// The native core: ingestion adapters, the normalizer + heuristic risk
// engine, the SQLite store and git integration. The desktop shell and the
// CLI both drive it through api::handle; behavior is pinned by the fixture
// contract in the repo-root fixtures/ (cargo test).

pub mod adapters;
pub mod api;
pub mod config;
pub mod demo;
pub mod domain;
#[cfg(test)]
pub(crate) mod fixture;
pub mod git;
pub mod normalize;
pub mod paths;
pub mod policy;
pub mod rules;
pub mod scanner;
pub mod storage;
