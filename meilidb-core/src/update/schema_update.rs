use meilidb_schema::Schema;
use crate::{store, error::UnsupportedOperation, MResult};
use crate::update::{Update, next_update_id};

pub fn apply_schema_update(
    writer: &mut rkv::Writer,
    main_store: store::Main,
    new_schema: &Schema,
) -> MResult<()>
{
    if let Some(_) = main_store.schema(writer)? {
        return Err(UnsupportedOperation::SchemaAlreadyExists.into())
    }

    main_store.put_schema(writer, new_schema)
}

pub fn push_schema_update(
    writer: &mut rkv::Writer,
    updates_store: store::Updates,
    updates_results_store: store::UpdatesResults,
    schema: Schema,
) -> MResult<u64>
{
    let last_update_id = next_update_id(writer, updates_store, updates_results_store)?;

    let update = Update::SchemaUpdate(schema);
    let update_id = updates_store.put_update(writer, last_update_id, &update)?;

    Ok(last_update_id)
}
