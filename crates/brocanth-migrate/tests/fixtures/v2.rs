// The models after: `user` is now `account`, `post.title` is now
// `post.headline`, `post.body` is new and `post.views` is gone. The ids did
// not move, so the first two are renames.

pub mod account {
    use brocanth_model::prelude::*;

    #[brocanth_model::ids]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "account")]
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

    #[brocanth_model::validates(headline(presence))]
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
        pub headline: String,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ25")]
        pub body: Option<String>,
        #[sea_orm(belongs_to, from = "user_id", to = "id")]
        pub author: BelongsTo<super::account::Entity>,
    }
}
