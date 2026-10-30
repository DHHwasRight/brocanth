//! The ids `brocanth migrate` reads are in the source text and nowhere else.
//! These models carry them and must compile into ordinary SeaORM entities.

mod article {
    use brocanth_model::prelude::*;

    #[brocanth_model::ids]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "article")]
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1A")]
    pub struct Model {
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1B")]
        #[sea_orm(primary_key)]
        pub id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1C")]
        pub headline: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

mod note {
    use brocanth_model::prelude::*;

    // `validates` takes the ids off too, so one attribute is enough.
    #[brocanth_model::validates(body(presence))]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "note")]
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1D")]
    pub struct Model {
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1E")]
        #[sea_orm(primary_key)]
        pub id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1F")]
        pub body: String,
    }
}

#[test]
fn a_model_with_ids_is_an_ordinary_entity() {
    use brocanth_model::sea_orm::{EntityName, IdenStatic, Iterable};

    assert_eq!(article::Entity.table_name(), "article");
    let columns: Vec<&str> = article::Column::iter().map(|c| c.as_str()).collect();
    assert_eq!(columns, ["id", "headline"]);
    let columns: Vec<&str> = note::Column::iter().map(|c| c.as_str()).collect();
    assert_eq!(columns, ["id", "body"]);
}
