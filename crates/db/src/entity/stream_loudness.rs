use sea_orm::entity::prelude::*;

/// An EBU R128 loudness measurement of an audio track.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "stream_loudness")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub stream_id: i64,
    /// The version of the analysis that measured it, as written: `0.02`.
    pub analysis_version: String,
    /// Integrated loudness, in LUFS.
    pub integrated: f64,
    /// Loudness range, in LU.
    pub loudness_range: f64,
    /// True peak, in dBTP.
    pub true_peak: f64,
    /// The gating threshold the integration used, in LUFS.
    pub threshold: f64,
    /// The gain, in dB, a player applies to bring the track to the target loudness.
    pub gain_offset: f64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::media_stream::Entity",
        from = "Column::StreamId",
        to = "super::media_stream::Column::Id",
        on_delete = "Cascade"
    )]
    MediaStream,
}

impl Related<super::media_stream::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaStream.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
