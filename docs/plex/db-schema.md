# Plex library database schema

Reference extracted from the pristine template database shipped with Plex Media Server (`files/Resources/com.plexapp.plugins.library.db`), not from a populated install — so it carries no personal data.

**Generated** — regenerate with `python tools/dump_plex_schema.py`.

This is a *reference*, not a target. klapz designs its own schema; this document exists to understand how Plex models its entities and which columns feed which API response attributes. See the notes at the end for the traps it reveals.

71 tables in total; 19 internal SQLite, FTS and R-tree tables omitted.

## Contents

- [`library_sections`](#library-sections)
- [`section_locations`](#section-locations)
- [`metadata_items`](#metadata-items)
- [`media_items`](#media-items)
- [`media_parts`](#media-parts)
- [`media_streams`](#media-streams)
- [`stream_types`](#stream-types)
- [`tags`](#tags)
- [`taggings`](#taggings)
- [`accounts`](#accounts)
- [`metadata_item_settings`](#metadata-item-settings)
- [`metadata_item_views`](#metadata-item-views)
- [`play_queues`](#play-queues)
- [`play_queue_items`](#play-queue-items)
- [`play_queue_generators`](#play-queue-generators)
- [`library_section_permissions`](#library-section-permissions)
- [`devices`](#devices)
- [`preferences`](#preferences)
- [`blobs`](#blobs)
- [`cloudsync_files`](#cloudsync-files)
- [`directories`](#directories)
- [`external_metadata_items`](#external-metadata-items)
- [`external_metadata_sources`](#external-metadata-sources)
- [`library_timeline_entries`](#library-timeline-entries)
- [`locatables`](#locatables)
- [`location_places`](#location-places)
- [`locations`](#locations)
- [`media_grabs`](#media-grabs)
- [`media_item_settings`](#media-item-settings)
- [`media_metadata_mappings`](#media-metadata-mappings)
- [`media_part_settings`](#media-part-settings)
- [`media_provider_resources`](#media-provider-resources)
- [`media_stream_settings`](#media-stream-settings)
- [`media_subscriptions`](#media-subscriptions)
- [`metadata_item_accounts`](#metadata-item-accounts)
- [`metadata_item_clusterings`](#metadata-item-clusterings)
- [`metadata_item_clusters`](#metadata-item-clusters)
- [`metadata_relations`](#metadata-relations)
- [`plugin_permissions`](#plugin-permissions)
- [`plugin_prefixes`](#plugin-prefixes)
- [`plugins`](#plugins)
- [`schema_migrations`](#schema-migrations)
- [`statistics_bandwidth`](#statistics-bandwidth)
- [`statistics_media`](#statistics-media)
- [`sync_schema_versions`](#sync-schema-versions)
- [`synced_ancestor_items`](#synced-ancestor-items)
- [`synced_library_sections`](#synced-library-sections)
- [`synced_metadata_items`](#synced-metadata-items)
- [`synced_play_queue_generators`](#synced-play-queue-generators)
- [`synchronization_files`](#synchronization-files)
- [`versioned_metadata_items`](#versioned-metadata-items)
- [`view_settings`](#view-settings)
- [Notes](#notes)

## `library_sections`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_id` | `INTEGER` | — |
| `name` | `varchar(255)` | — |
| `name_sort` | `varchar(255)` | — |
| `section_type` | `INTEGER` | — |
| `language` | `varchar(255)` | — |
| `agent` | `varchar(255)` | — |
| `scanner` | `varchar(255)` | — |
| `user_thumb_url` | `varchar(255)` | — |
| `user_art_url` | `varchar(255)` | — |
| `user_theme_music_url` | `varchar(255)` | — |
| `public` | `boolean` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `scanned_at` | `datetime` | — |
| `display_secondary_level` | `boolean` | — |
| `user_fields` | `varchar(255)` | — |
| `query_xml` | `TEXT` | — |
| `query_type` | `INTEGER` | — |
| `uuid` | `varchar(255)` | — |
| `changed_at` | `integer(8)` | default `0` |

3 index(es).

## `section_locations`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_section_id` | `INTEGER` | — |
| `root_path` | `varchar(255)` | — |
| `available` | `boolean` | default `'t'` |
| `scanned_at` | `datetime` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

No indexes.

## `metadata_items`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_section_id` | `INTEGER` | — |
| `parent_id` | `INTEGER` | — |
| `metadata_type` | `INTEGER` | — |
| `guid` | `varchar(255)` | — |
| `media_item_count` | `INTEGER` | — |
| `title` | `varchar(255)` | — |
| `title_sort` | `varchar(255)` | — |
| `original_title` | `varchar(255)` | — |
| `studio` | `varchar(255)` | — |
| `rating` | `float` | — |
| `rating_count` | `INTEGER` | — |
| `tagline` | `varchar(255)` | — |
| `summary` | `TEXT` | — |
| `trivia` | `TEXT` | — |
| `quotes` | `TEXT` | — |
| `content_rating` | `varchar(255)` | — |
| `content_rating_age` | `INTEGER` | — |
| `"index"` | `INTEGER` | — |
| `absolute_index` | `INTEGER` | — |
| `duration` | `INTEGER` | — |
| `user_thumb_url` | `varchar(255)` | — |
| `user_art_url` | `varchar(255)` | — |
| `user_banner_url` | `varchar(255)` | — |
| `user_music_url` | `varchar(255)` | — |
| `user_fields` | `varchar(255)` | — |
| `tags_genre` | `varchar(255)` | — |
| `tags_collection` | `varchar(255)` | — |
| `tags_director` | `varchar(255)` | — |
| `tags_writer` | `varchar(255)` | — |
| `tags_star` | `varchar(255)` | — |
| `originally_available_at` | `datetime` | — |
| `available_at` | `datetime` | — |
| `expires_at` | `datetime` | — |
| `refreshed_at` | `datetime` | — |
| `year` | `INTEGER` | — |
| `added_at` | `datetime` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `deleted_at` | `datetime` | — |
| `tags_country` | `varchar(255)` | — |
| `extra_data` | `varchar(255)` | — |
| `hash` | `varchar(255)` | — |
| `audience_rating` | `float` | — |
| `changed_at` | `integer(8)` | default `0` |
| `resources_changed_at` | `integer(8)` | default `0` |

18 index(es).

## `media_items`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_section_id` | `INTEGER` | — |
| `section_location_id` | `INTEGER` | — |
| `metadata_item_id` | `INTEGER` | — |
| `type_id` | `INTEGER` | — |
| `width` | `INTEGER` | — |
| `height` | `INTEGER` | — |
| `size` | `integer(8)` | — |
| `duration` | `INTEGER` | — |
| `bitrate` | `INTEGER` | — |
| `container` | `varchar(255)` | — |
| `video_codec` | `varchar(255)` | — |
| `audio_codec` | `varchar(255)` | — |
| `display_aspect_ratio` | `float` | — |
| `frames_per_second` | `float` | — |
| `audio_channels` | `INTEGER` | — |
| `interlaced` | `boolean` | — |
| `source` | `varchar(255)` | — |
| `hints` | `varchar(255)` | — |
| `display_offset` | `INTEGER` | — |
| `settings` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `optimized_for_streaming` | `boolean` | — |
| `deleted_at` | `datetime` | — |
| `media_analysis_version` | `INTEGER` | default `0` |
| `sample_aspect_ratio` | `float` | — |
| `extra_data` | `varchar(255)` | — |
| `proxy_type` | `INTEGER` | — |
| `channel_id` | `INTEGER` | — |
| `begins_at` | `datetime` | — |
| `ends_at` | `datetime` | — |

8 index(es).

## `media_parts`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `media_item_id` | `INTEGER` | — |
| `directory_id` | `INTEGER` | — |
| `hash` | `varchar(255)` | — |
| `open_subtitle_hash` | `varchar(255)` | — |
| `file` | `varchar(255)` | — |
| `"index"` | `INTEGER` | — |
| `size` | `integer(8)` | — |
| `duration` | `INTEGER` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `deleted_at` | `datetime` | — |
| `extra_data` | `varchar(255)` | — |

6 index(es).

## `media_streams`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `stream_type_id` | `INTEGER` | — |
| `media_item_id` | `INTEGER` | — |
| `url` | `varchar(255)` | — |
| `codec` | `varchar(255)` | — |
| `language` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `"index"` | `INTEGER` | — |
| `media_part_id` | `INTEGER` | — |
| `channels` | `INTEGER` | — |
| `bitrate` | `INTEGER` | — |
| `url_index` | `INTEGER` | — |
| `"default"` | `boolean` | default `0` |
| `forced` | `boolean` | default `0` |
| `extra_data` | `varchar(255)` | — |

3 index(es).

## `stream_types`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `name` | `varchar(255)` | — |

No indexes.

## `tags`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `metadata_item_id` | `INTEGER` | — |
| `tag` | `varchar(255)` | — |
| `tag_type` | `INTEGER` | — |
| `user_thumb_url` | `varchar(255)` | — |
| `user_art_url` | `varchar(255)` | — |
| `user_music_url` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `tag_value` | `INTEGER` | — |
| `extra_data` | `varchar(255)` | — |
| `key` | `varchar(255)` | — |
| `parent_id` | `INTEGER` | — |

5 index(es).

## `taggings`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `metadata_item_id` | `INTEGER` | — |
| `tag_id` | `INTEGER` | — |
| `"index"` | `INTEGER` | — |
| `text` | `varchar(255)` | — |
| `time_offset` | `INTEGER` | — |
| `end_time_offset` | `INTEGER` | — |
| `thumb_url` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `extra_data` | `varchar(255)` | — |

2 index(es).

## `accounts`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `name` | `varchar(255)` | — |
| `hashed_password` | `varchar(255)` | — |
| `salt` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `default_audio_language` | `varchar(255)` | — |
| `default_subtitle_language` | `varchar(255)` | — |
| `auto_select_subtitle` | `boolean` | default `'t'` |
| `auto_select_audio` | `boolean` | default `'t'` |

1 index(es).

## `metadata_item_settings`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `guid` | `varchar(255)` | — |
| `rating` | `float` | — |
| `view_offset` | `INTEGER` | — |
| `view_count` | `INTEGER` | — |
| `last_viewed_at` | `datetime` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `skip_count` | `INTEGER` | default `0` |
| `last_skipped_at` | `datetime` | default `NULL` |
| `changed_at` | `integer(8)` | default `0` |
| `extra_data` | `varchar(255)` | — |

8 index(es).

## `metadata_item_views`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `guid` | `varchar(255)` | — |
| `metadata_type` | `INTEGER` | — |
| `library_section_id` | `INTEGER` | — |
| `grandparent_title` | `varchar(255)` | — |
| `parent_index` | `INTEGER` | — |
| `parent_title` | `varchar(255)` | — |
| `"index"` | `INTEGER` | — |
| `title` | `varchar(255)` | — |
| `thumb_url` | `varchar(255)` | — |
| `viewed_at` | `datetime` | — |
| `grandparent_guid` | `varchar(255)` | — |
| `originally_available_at` | `datetime` | — |

4 index(es).

## `play_queues`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `client_identifier` | `varchar(255)` | — |
| `account_id` | `INTEGER` | — |
| `playlist_id` | `INTEGER` | — |
| `sync_item_id` | `INTEGER` | — |
| `play_queue_generator_id` | `INTEGER` | — |
| `generator_start_index` | `INTEGER` | — |
| `generator_end_index` | `INTEGER` | — |
| `generator_items_count` | `INTEGER` | — |
| `generator_ids` | `BLOB` | — |
| `seed` | `INTEGER` | — |
| `current_play_queue_item_id` | `INTEGER` | — |
| `last_added_play_queue_item_id` | `INTEGER` | — |
| `version` | `INTEGER` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `metadata_type` | `INTEGER` | — |
| `total_items_count` | `INTEGER` | — |
| `generator_generator_ids` | `BLOB` | — |

4 index(es).

## `play_queue_items`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `play_queue_id` | `INTEGER` | — |
| `metadata_item_id` | `INTEGER` | — |
| `"order"` | `float` | — |
| `up_next` | `boolean` | — |
| `play_queue_generator_id` | `INTEGER` | — |

3 index(es).

## `play_queue_generators`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `playlist_id` | `INTEGER` | — |
| `metadata_item_id` | `INTEGER` | — |
| `uri` | `varchar(255)` | — |
| `limit` | `INTEGER` | — |
| `continuous` | `boolean` | — |
| `"order"` | `float` | — |
| `created_at` | `datetime` | not null |
| `updated_at` | `datetime` | not null |
| `changed_at` | `integer(8)` | default `0` |
| `recursive` | `boolean` | — |
| `type` | `INTEGER` | — |
| `extra_data` | `varchar(255)` | — |

4 index(es).

## `library_section_permissions`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_section_id` | `INTEGER` | — |
| `account_id` | `INTEGER` | — |
| `permission` | `INTEGER` | — |

2 index(es).

## `devices`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `identifier` | `varchar(255)` | — |
| `name` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

1 index(es).

## `preferences`

On the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `name` | `varchar(255)` | — |
| `value` | `varchar(255)` | — |

1 index(es).

## `blobs`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `blob` | `BLOB` | — |
| `linked_type` | `varchar(255)` | — |
| `linked_id` | `INTEGER` | — |
| `linked_guid` | `varchar(255)` | — |
| `created_at` | `datetime` | — |

3 index(es).

## `cloudsync_files`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `device_identifier` | `varchar(255)` | — |
| `original_url` | `varchar(255)` | — |
| `provider` | `varchar(255)` | — |
| `new_key` | `varchar(255)` | — |
| `query_string` | `varchar(255)` | — |
| `extra_data` | `varchar(255)` | — |

1 index(es).

## `directories`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_section_id` | `INTEGER` | — |
| `parent_directory_id` | `INTEGER` | — |
| `path` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `deleted_at` | `datetime` | — |

2 index(es).

## `external_metadata_items`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | — |
| `external_metadata_source_id` | `INTEGER` | — |
| `library_section_id` | `INTEGER` | — |
| `metadata_type` | `INTEGER` | — |
| `guid` | `varchar(255)` | — |
| `title` | `varchar(255)` | — |
| `parent_title` | `varchar(255)` | — |
| `year` | `INTEGER` | — |
| `added_at` | `INTEGER` | — |
| `updated_at` | `INTEGER` | — |

4 index(es).

## `external_metadata_sources`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `uri` | `varchar(255)` | — |
| `source_title` | `varchar(255)` | — |
| `user_title` | `varchar(255)` | — |
| `online` | `INTEGER` | — |

2 index(es).

## `library_timeline_entries`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `library_section_id` | `INTEGER` | — |
| `metadata_item_id` | `INTEGER` | — |
| `state` | `INTEGER` | — |
| `updated_at` | `datetime` | — |

4 index(es).

## `locatables`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `location_id` | `INTEGER` | not null |
| `locatable_id` | `INTEGER` | not null |
| `locatable_type` | `VARCHAR(255)` | not null |
| `created_at` | `DATETIME` | — |
| `updated_at` | `DATETIME` | — |
| `extra_data` | `VARCHAR(255)` | — |
| `geocoding_version` | `INTEGER` | — |

4 index(es).

## `location_places`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `location_id` | `INTEGER` | — |
| `guid` | `VARCHAR(255)` | not null |

2 index(es).

## `locations`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INT` | — |
| `lat_min` | `REAL` | — |
| `lat_max` | `REAL` | — |
| `lon_min` | `REAL` | — |
| `lon_max` | `REAL` | — |

No indexes.

## `media_grabs`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `uuid` | `varchar(255)` | — |
| `status` | `INTEGER` | — |
| `error` | `INTEGER` | — |
| `metadata_item_id` | `INTEGER` | — |
| `media_subscription_id` | `INTEGER` | — |
| `extra_data` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

3 index(es).

## `media_item_settings`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `media_item_id` | `INTEGER` | — |
| `settings` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

2 index(es).

## `media_metadata_mappings`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `media_guid` | `varchar(255)` | — |
| `metadata_guid` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

No indexes.

## `media_part_settings`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `media_part_id` | `INTEGER` | — |
| `selected_audio_stream_id` | `INTEGER` | — |
| `selected_subtitle_stream_id` | `INTEGER` | — |
| `settings` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `changed_at` | `integer(8)` | default `0` |

3 index(es).

## `media_provider_resources`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `parent_id` | `INTEGER` | — |
| `type` | `INTEGER` | — |
| `status` | `INTEGER` | — |
| `state` | `INTEGER` | — |
| `identifier` | `varchar(255)` | — |
| `protocol` | `varchar(255)` | — |
| `uri` | `varchar(255)` | — |
| `uuid` | `varchar(255)` | — |
| `extra_data` | `varchar(255)` | — |
| `last_seen_at` | `datetime` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |
| `data` | `BLOB` | — |

4 index(es).

## `media_stream_settings`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `media_stream_id` | `INTEGER` | — |
| `extra_data` | `VARCHAR(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

2 index(es).

## `media_subscriptions`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `"order"` | `float` | — |
| `metadata_type` | `INTEGER` | — |
| `target_metadata_item_id` | `INTEGER` | — |
| `target_library_section_id` | `INTEGER` | — |
| `target_section_location_id` | `INTEGER` | — |
| `extra_data` | `varchar(255)` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

2 index(es).

## `metadata_item_accounts`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `metadata_item_id` | `INTEGER` | — |

2 index(es).

## `metadata_item_clusterings`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `metadata_item_id` | `INTEGER` | — |
| `metadata_item_cluster_id` | `INTEGER` | — |
| `"index"` | `INTEGER` | — |
| `version` | `INTEGER` | — |

3 index(es).

## `metadata_item_clusters`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `zoom_level` | `INTEGER` | — |
| `library_section_id` | `INTEGER` | — |
| `title` | `varchar(255)` | — |
| `count` | `INTEGER` | — |
| `starts_at` | `datetime` | — |
| `ends_at` | `datetime` | — |
| `extra_data` | `varchar(255)` | — |

3 index(es).

## `metadata_relations`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `metadata_item_id` | `INTEGER` | — |
| `related_metadata_item_id` | `INTEGER` | — |
| `relation_type` | `INTEGER` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

3 index(es).

## `plugin_permissions`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `plugin_id` | `INTEGER` | — |
| `account_id` | `INTEGER` | — |
| `permission` | `INTEGER` | — |

2 index(es).

## `plugin_prefixes`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `plugin_id` | `INTEGER` | — |
| `name` | `varchar(255)` | — |
| `prefix` | `varchar(255)` | — |
| `art_url` | `varchar(255)` | — |
| `thumb_url` | `varchar(255)` | — |
| `titlebar_url` | `varchar(255)` | — |
| `share` | `boolean` | — |
| `has_store_services` | `boolean` | — |
| `prefs` | `boolean` | — |

1 index(es).

## `plugins`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `identifier` | `varchar(255)` | — |
| `framework_version` | `INTEGER` | — |
| `access_count` | `INTEGER` | — |
| `installed_at` | `datetime` | — |
| `accessed_at` | `datetime` | — |
| `modified_at` | `datetime` | — |

No indexes.

## `schema_migrations`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `version` | `varchar(255)` | not null |
| `rollback_sql` | `TEXT` | default `NULL` |
| `optimize_on_rollback` | `boolean` | default `NULL` |

1 index(es).

## `statistics_bandwidth`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `device_id` | `INTEGER` | — |
| `timespan` | `INTEGER` | — |
| `at` | `datetime` | — |
| `lan` | `boolean` | — |
| `bytes` | `integer(8)` | — |

3 index(es).

## `statistics_media`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `device_id` | `INTEGER` | — |
| `timespan` | `INTEGER` | — |
| `at` | `datetime` | — |
| `metadata_type` | `INTEGER` | — |
| `count` | `INTEGER` | — |
| `duration` | `INTEGER` | — |

3 index(es).

## `sync_schema_versions`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `version` | `INTEGER` | — |
| `changed_at` | `integer(8)` | — |

1 index(es).

## `synced_ancestor_items`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `sync_list_id` | `integer(8)` | — |
| `metadata_item_id` | `INTEGER` | — |
| `changed_at` | `integer(8)` | — |
| `reference_count` | `INTEGER` | — |
| `first_packaged_at` | `integer(8)` | — |
| `parent_id` | `INTEGER` | — |
| `state` | `INTEGER` | — |

7 index(es).

## `synced_library_sections`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `sync_list_id` | `integer(8)` | — |
| `library_section_id` | `INTEGER` | — |
| `changed_at` | `integer(8)` | — |
| `reference_count` | `INTEGER` | — |
| `first_packaged_at` | `integer(8)` | — |
| `state` | `INTEGER` | — |

6 index(es).

## `synced_metadata_items`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `sync_list_id` | `integer(8)` | — |
| `sync_item_id` | `integer(8)` | — |
| `metadata_item_id` | `INTEGER` | — |
| `changed_at` | `integer(8)` | — |
| `first_packaged_at` | `integer(8)` | — |
| `state` | `INTEGER` | — |
| `state_context` | `INTEGER` | — |
| `selected_media_id` | `INTEGER` | — |
| `selected_part_id` | `INTEGER` | — |
| `media_decision` | `INTEGER` | — |
| `file_size` | `integer(8)` | — |
| `media_analysis_extra_data` | `varchar(255)` | — |
| `parent_id` | `INTEGER` | — |
| `library_section_id` | `INTEGER` | — |

9 index(es).

## `synced_play_queue_generators`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `sync_list_id` | `integer(8)` | — |
| `sync_item_id` | `integer(8)` | — |
| `playlist_id` | `INTEGER` | — |
| `play_queue_generator_id` | `INTEGER` | — |
| `changed_at` | `integer(8)` | — |
| `state` | `INTEGER` | — |
| `state_context` | `INTEGER` | — |
| `first_packaged_at` | `integer(8)` | — |

7 index(es).

## `synchronization_files`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `client_identifier` | `varchar(255)` | — |
| `sync_list_id` | `integer(8)` | — |
| `sync_item_id` | `integer(8)` | — |
| `item_uri` | `varchar(255)` | — |
| `num_parts` | `INTEGER` | — |
| `state` | `INTEGER` | — |
| `state_context` | `INTEGER` | — |
| `extra_data` | `varchar(255)` | — |

4 index(es).

## `versioned_metadata_items`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `metadata_item_id` | `INTEGER` | — |
| `generator_id` | `INTEGER` | — |
| `target_tag_id` | `INTEGER` | — |
| `state` | `INTEGER` | — |
| `state_context` | `INTEGER` | — |
| `selected_media_id` | `INTEGER` | — |
| `version_media_id` | `INTEGER` | — |
| `media_decision` | `INTEGER` | — |
| `file_size` | `integer(8)` | — |

5 index(es).

## `view_settings`

Not on the API read path.

| Column | Type | Constraints |
| --- | --- | --- |
| `id` | `INTEGER` | primary key, not null |
| `account_id` | `INTEGER` | — |
| `client_type` | `varchar(255)` | — |
| `view_group` | `varchar(255)` | — |
| `view_id` | `INTEGER` | — |
| `sort_id` | `INTEGER` | — |
| `sort_asc` | `boolean` | — |
| `created_at` | `datetime` | — |
| `updated_at` | `datetime` | — |

2 index(es).

## Notes

What this schema tells us, and why klapz does not clone it:

- **Types are integers, the API emits strings.** `metadata_items.metadata_type` and `library_sections.section_type` are integer codes, while the API exposes `type="movie"`. The mapping lives at the binding boundary.
- **Timestamps are `datetime`, the API emits epoch seconds.** Another conversion the binding owns; `core` should hold one unambiguous time type.
- **Packed columns.** `extra_data` and `user_fields` hold encoded key/value blobs rather than real columns. They are unqueryable and version-fragile — the kind of design decision we inherit nothing from.
- **`index` is a column name**, and a reserved SQL word. A reminder that identifiers from this schema cannot be interpolated naively into queries.
- **Denormalized tag columns.** `tags_genre`, `tags_director`, `tags_star` duplicate what `taggings` already models, as comma-joined strings for display speed.
- **`changed_at` / `resources_changed_at`** are change counters driving sync, not timestamps in the usual sense.
