# MediaContainer serialization

## Contents

- [The shape of a response](#the-shape-of-a-response)
- [Why one derive cannot serve both formats](#why-one-derive-cannot-serve-both-formats)
- [The chosen approach](#the-chosen-approach)
- [Absent fields](#absent-fields)
- [Scalar encodings](#scalar-encodings)
- [Element names versus JSON keys](#element-names-versus-json-keys)
- [Content negotiation](#content-negotiation)
- [Pagination](#pagination)
- [What must be confirmed from fixtures](#what-must-be-confirmed-from-fixtures)

## The shape of a response

Every Plex response is wrapped in a `MediaContainer`. XML is the default; JSON is returned only
when the client asks with `Accept: application/json`.

In XML, scalars are **attributes** on the element and relations are **child elements**:

```xml
<MediaContainer size="2" allowSync="0" identifier="com.plexapp.plugins.library">
  <Directory key="1" type="movie" title="Movies" refreshing="0" updatedAt="1680000000">
    <Location id="1" path="/media/movies"/>
  </Directory>
</MediaContainer>
```

In JSON, the container becomes a single wrapping object and the children become arrays under
capitalized keys:

```json
{
  "MediaContainer": {
    "size": 2,
    "identifier": "com.plexapp.plugins.library",
    "Directory": [
      { "key": "1", "type": "movie", "title": "Movies",
        "Location": [{ "id": 1, "path": "/media/movies" }] }
    ]
  }
}
```

The two are structurally isomorphic: an XML attribute is a scalar JSON key, and an XML child
element is an entry in a capitalized JSON array. That isomorphism is what makes a single data
model viable — and it is the property the approach below exploits.

## Why one derive cannot serve both formats

The obvious plan is one struct with `#[derive(Serialize)]`, fed to `quick-xml` for XML and
`serde_json` for JSON. It does not work, and it is worth understanding why before trying.

`quick-xml` distinguishes attributes from child elements by a naming convention in the serde
data model: a field renamed to `@key` is written as an attribute, otherwise it becomes a child
element. That rename is static, baked into the derive. So the same struct that produces
`key="1"` in XML produces `"@key": "1"` in JSON — with a leading `@` that no Plex client
expects.

Writing two parallel structs per entity is the other obvious escape, and it is worse: with
`Metadata` alone carrying 67 fields, every one of them would have to be kept in sync by hand
across both copies, forever.

## The chosen approach

**JSON is the canonical representation; XML is emitted by walking it.**

Each response type derives `Serialize` once, with plain camelCase field names — the JSON form,
with no `@` anywhere. XML is then produced by a single generic emitter that walks the serde data
model and applies the isomorphism:

- a scalar field becomes an attribute;
- a field whose value is a sequence becomes a run of child elements;
- the key names the element.

```rust
/// Emits Plex-flavored XML from any `Serialize` value.
///
/// Relies on the attribute/child isomorphism described above, so response types stay
/// declared once in their JSON form. The element name for a child run comes from the
/// map key, except for metadata items — see `element_name_for`.
pub fn to_plex_xml<T: Serialize>(value: &T, root: &str) -> Result<String, XmlError>
```

This buys three things that matter at this project's scale: one declaration per entity, no `@`
leaking into JSON, and no risk of the two formats drifting apart as fields are added. The cost is
one non-trivial emitter to write and test thoroughly — which is a far better trade than 60
hand-maintained type pairs.

Test the emitter against captured fixtures, not against its own output. It is the single piece of
code every endpoint depends on, so it deserves the densest unit tests in the repo.

## Absent fields

**`skip_serializing_if = "Option::is_none"` on every optional field.** This is not a style
preference. Plex *omits* absent attributes entirely; emitting `viewOffset=""` in XML or
`"viewOffset": null` in JSON diverges from real server behavior, and clients do read the
difference — an unwatched item has no `viewOffset` at all, which is not the same as one at zero.

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub rating_key: RatingKey,
    pub title: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_offset: Option<Millis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "Media")]
    pub media: Vec<Media>,
}
```

Empty collections are skipped too: Plex does not emit an empty `<Media/>` run, and in JSON an
empty array is not the same signal as an absent key.

Because missing attributes are the normal case rather than the exception, make the absent-field
behavior an explicit test per endpoint. A field wrongly present is invisible in casual testing
and breaks a specific client months later.

## Scalar encodings

Confirmed against real responses:

| Domain value | XML | Notes |
| --- | --- | --- |
| Boolean | `0` / `1` | Never `true`/`false`. Applies to `allowSync`, `refreshing`, `hidden`, `filters`, `content`, `directory`… |
| Timestamp | epoch seconds, numeric | `updatedAt`, `createdAt`, `addedAt`, `scannedAt`, `lastViewedAt` |
| Duration / offset | milliseconds, numeric | `duration`, `viewOffset` |
| Date | `YYYY-MM-DD` | `originallyAvailableAt` |

Booleans therefore need a dedicated type rather than `bool`, since `bool` serializes to
`true`/`false` in both formats:

```rust
/// Plex encodes booleans as 0/1 rather than true/false.
///
/// Whether JSON mirrors that or uses real booleans varies by field, so the JSON form is
/// pinned per field from fixtures rather than assumed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlexBool(bool);
```

Be careful with numeric-looking identifiers. `ratingKey` is documented as a **string** in the
official spec even though its value is digits, while `size` is a number. Do not normalize these
into what looks consistent — match what the server actually emits, field by field, because
client parsers are written against the real thing.

## Element names versus JSON keys

The one place where the isomorphism needs help. In XML, a metadata item's element name depends on
its kind — `<Video>` for a movie or episode, `<Directory>` for a show or season, `<Track>` for
music. In JSON, those same items all collapse into a single `"Metadata"` array.

So the mapping is not a pure function of the key: the emitter needs the item's `type` to pick the
element name. Carry that as an explicit hint rather than inferring it from the payload, and keep
the mapping table in one place.

The mapping is irregular across endpoints, confirmed by capture:

| Endpoint | XML child | JSON key |
| --- | --- | --- |
| `/library/sections` | `<Directory>` | `"Directory"` |
| `/library/sections/{id}/all` | `<Video>` | `"Metadata"` |
| `/library/metadata/{id}` | `<Video>` | `"Metadata"` |

So `<Directory>` maps to `"Directory"` in a section listing and to `"Metadata"` when it is a show
or season inside a metadata listing. The same element name, two different keys, decided by
context.

There is no rule to derive here — it is per-endpoint behavior that must be read off captures and
encoded explicitly. Do not invent a clever generalization; write the table.

## Content negotiation

Measured against a real server (1.43.5), the rule is narrow and exact:

| Request | Response `Content-Type` |
| --- | --- |
| no `Accept` | `text/xml;charset=utf-8` |
| `Accept: application/json` | `application/json` |
| `Accept: application/json, text/plain, */*` | `application/json` |
| `Accept: text/xml` | `text/xml;charset=utf-8` |
| `Accept: */*` | `text/xml;charset=utf-8` |
| `Accept: text/html,*/*;q=0.8` | `text/xml;charset=utf-8` |
| `Content-Type: application/json` | `text/xml;charset=utf-8` |
| `?format=json` | `text/xml;charset=utf-8` |

So: **JSON if and only if `Accept` contains `application/json`; XML otherwise.** Two consequences
worth stating because both are easy to assume wrong:

- `Content-Type` on the request has no effect. It describes the request body, and these are GETs.
- There is **no `?format=` query override**. Do not implement one; it would be a behavior Plex
  does not have.

Note the exact XML content type: `text/xml;charset=utf-8`, not `application/xml`. Clients have
been written against the real string.

In practice clients overwhelmingly ask for JSON — the bundled Plex Web App sends
`accept: application/json, text/plain, */*` — so JSON is the path that must be best tested, even
though XML is the server's default.

Model the decision as an extractor so it appears in handler signatures and is impossible to
forget:

```rust
#[derive(Debug, Clone, Copy)]
pub enum ResponseFormat {
    Xml,
    Json,
}
```

Then a response wrapper that owns the serialization choice and sets the matching
`Content-Type` — handlers never do this themselves:

```rust
pub struct PlexResponse<T> {
    body: T,
    format: ResponseFormat,
}

impl<T: Serialize> IntoResponse for PlexResponse<T> { /* ... */ }
```

Clients in the wild also pass `?format=json` or send a sloppy `Accept` (`*/*`, several values
with q-factors). Treat an unparseable or absent `Accept` as XML, since that is the server's
default, and accept the explicit query override.

## Pagination

The client requests a window with `X-Plex-Container-Start` and `X-Plex-Container-Size` (as query
parameters, in practice). The server answers with **two** response headers, not three:

```
X-Plex-Container-Start: 6
X-Plex-Container-Total-Size: 14
```

There is no `X-Plex-Container-Size` response header — do not emit one. The same numbers also
appear as container attributes, and the two must agree.

The distinction that bites, shown by a real response to `Start=6&Size=24` over a 14-item set:

```xml
<MediaContainer size="8" totalSize="14" offset="6" ...>
```

`size` is the number of children **in this response** (8, because only 8 remained after offset
6), `offset` echoes the requested start, and `totalSize` is the count over the **whole filtered
set**. A server that returns the same value for `size` and `totalSize` makes clients loop
forever. Caps on the requested size are in `security.md`.

## Confirmed by capture

A live reference server is available at `127.0.0.1:32400`; `tools/capture_plex.py` records
responses and regenerates `docs/wire-format.md`, which holds the authoritative per-field type
map. What the first captures settled:

**JSON scalar types are per-field and not inferable.** Both of these are numeric values, and they
disagree:

```json
{ "ratingKey": "407618", "Media": [{ "id": 123456 }] }
```

`ratingKey` is a string, `Media.id` is a number. Likewise `year`, `duration`, `addedAt`,
`updatedAt`, `viewCount`, `contentRatingAge` are numbers; `rating` and `audienceRating` are
floats; `originallyAvailableAt` is a `"YYYY-MM-DD"` string; `videoFrameRate` is a string.

**Booleans are converted inconsistently.** XML always uses `0`/`1`, but JSON does not pick one
representation:

| Field | XML | JSON |
| --- | --- | --- |
| `MediaContainer/allowSync` | `"1"` | `true` |
| `Meta/Type/active`, `Meta/Type/Sort/active` | `"1"` | `true` |
| `Metadata/hasPremiumPrimaryExtra` | `"1"` | `"1"` (string) |
| `Meta/Type/Sort/default`, `defaultDirection` | `"1"` | `"1"` (string) |

So a single `PlexBool` type cannot decide its own JSON form — the representation is a property of
the field, pinned from captures.

**Some children are objects, not arrays.** In XML every child is an element, but in JSON:

- `Meta` and `Metadata/UltraBlurColors` are **objects**;
- `Metadata`, `Media`, `Part`, `Stream`, `Genre`, `Country`, `Director`, `Writer`, `Role`,
  `Producer`, `Collection`, `Guid`, `Image`, `Location`, `Filter`, `Sort`, `Field` are **arrays**
  — including when they hold a single element.

Singleton-versus-collection is therefore per-key, not a function of how many children exist.

**Omission is the normal case.** In one 8-item listing: `viewCount` and `lastViewedAt` present on
1 item, `titleSort` on 3, `contentRatingAge` and `slug` on 5, `rating` on 7. Any model that
emits these unconditionally is wrong.

**XML escapes non-ASCII as numeric character references** despite declaring `encoding="UTF-8"` —
`Année` is emitted as `Ann&#233;e`. Byte-for-byte fixture comparison requires reproducing this.

Still open: attribute ordering within an element (no client is known to depend on it), and the
per-endpoint element-name table, which grows as endpoints are captured.
