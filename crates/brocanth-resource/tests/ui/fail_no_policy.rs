//! PLAN.md §12: "a build of `brocanth-resource` with no policy registered must
//! not compile".
//!
//! Everything else about this resource is in order. There is no policy, and
//! that alone is the error.

mod support;

use brocanth_resource::policy::Value;
use brocanth_resource::{ColumnOf, Failure, Resource, RowOf};
use support::{Post, Rows, Visitor};

pub struct Posts;

impl Resource for Posts {
    type Entity = Post;
    type Subject = Visitor;
    type Data = Rows;

    const NAME: &'static str = "posts";

    fn data(&self) -> &Rows {
        &Rows
    }

    fn parse_id(&self, _raw: &str) -> Result<Value, Failure> {
        Ok(Value::Int(1))
    }

    fn parse_write(&self, _body: &[u8]) -> Result<Vec<(ColumnOf<Self>, Value)>, Failure> {
        Ok(Vec::new())
    }

    fn encode_row(&self, _row: &RowOf<Self>, _out: &mut Vec<u8>) -> Result<(), Failure> {
        Ok(())
    }
}

fn main() {}
