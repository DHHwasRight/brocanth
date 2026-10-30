// The models before. Read as text by tests/migrations.rs, never compiled here.

pub mod user {
    use brocanth_model::prelude::*;

    #[brocanth_model::ids]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "user")]
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ10")]
    pub struct Model {
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ11")]
        #[sea_orm(primary_key)]
        pub id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ12")]
        pub name: String,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ13")]
        #[sea_orm(unique)]
        pub email: String,
        #[sea_orm(has_many)]
        pub posts: HasMany<super::post::Entity>,
    }
}

pub mod post {
    use brocanth_model::prelude::*;

    #[brocanth_model::validates(title(presence))]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "post")]
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ20")]
    pub struct Model {
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ21")]
        #[sea_orm(primary_key)]
        pub id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ22")]
        pub user_id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ23")]
        pub title: String,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ24")]
        pub views: i32,
        #[sea_orm(belongs_to, from = "user_id", to = "id")]
        pub author: BelongsTo<super::user::Entity>,
    }
}
