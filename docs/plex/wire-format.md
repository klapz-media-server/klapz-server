# Wire format

Per-field type map derived from a live reference server. **Generated** — regenerate with `PLEX_TOKEN=... python tools/capture_plex.py`.

Field names and types only; no captured values appear here. The raw bodies land in `captures/`, which is not committed because it carries library content and the token used to fetch it.

## Endpoints captured

| Endpoint | Status | Default `Content-Type` | With JSON `Accept` |
| --- | --: | --- | --- |
| `/identity` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections/{section}` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections/{section}/all` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections/{section}/all?includeMeta=1` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections/{section}/filters` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections/{section}/sorts` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/sections/{section}/firstCharacter?type=1` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/metadata/{item}` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/metadata/{item}/tree` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/library/metadata/{item}/related` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/hubs` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/hubs/continueWatching` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/status/sessions` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/playQueues` | 200 | `text/xml;charset=utf-8` | `application/json` |
| `/:/prefs` | 200 | `text/xml;charset=utf-8` | `application/json` |

## XML child elements per endpoint

Element names present in the XML response. Compare against the JSON keys below: the mapping is irregular and per-endpoint.

| Endpoint | Elements |
| --- | --- |
| `/identity` | `MediaContainer` |
| `/` | `Directory`, `MediaContainer` |
| `/library/sections` | `Directory`, `Location`, `MediaContainer` |
| `/library/sections/{section}` | `Directory`, `MediaContainer` |
| `/library/sections/{section}/all` | `Collection`, `Country`, `Director`, `Genre`, `Image`, `Media`, `MediaContainer`, `Part`, `Role`, `UltraBlurColors`, `Video`, `Writer` |
| `/library/sections/{section}/all?includeMeta=1` | `Collection`, `Country`, `Director`, `Field`, `FieldType`, `Filter`, `Genre`, `Image`, `Media`, `MediaContainer`, `Meta`, `Operator`, `Part`, `Role`, `Sort`, `Type`, `UltraBlurColors`, `Video`, `Writer` |
| `/library/sections/{section}/filters` | `Directory`, `MediaContainer` |
| `/library/sections/{section}/sorts` | `Directory`, `MediaContainer` |
| `/library/sections/{section}/firstCharacter?type=1` | `Directory`, `MediaContainer` |
| `/library/metadata/{item}` | `Country`, `Field`, `Genre`, `Guid`, `Image`, `Media`, `MediaContainer`, `Part`, `Stream`, `UltraBlurColors`, `Video` |
| `/library/metadata/{item}/tree` | `MediaContainer`, `MediaItem`, `MediaPart`, `MediaStream`, `MetadataItem` |
| `/library/metadata/{item}/related` | `MediaContainer` |
| `/hubs` | `AgeRating`, `Collection`, `CommonSenseMedia`, `Country`, `Director`, `Directory`, `Genre`, `Guid`, `Hub`, `Image`, `Location`, `Media`, `MediaContainer`, `Part`, `Producer`, `Rating`, `Role`, `Stream`, `UltraBlurColors`, `Video`, `Writer` |
| `/hubs/continueWatching` | `Collection`, `Country`, `Director`, `Genre`, `Guid`, `Hub`, `Image`, `Media`, `MediaContainer`, `Part`, `Producer`, `Rating`, `Role`, `Stream`, `UltraBlurColors`, `Video`, `Writer` |
| `/status/sessions` | `MediaContainer` |
| `/playQueues` | `MediaContainer` |
| `/:/prefs` | `MediaContainer`, `Setting` |

## JSON containers: object or array

A key listed as `object` is a single nested value in JSON even though XML represents it as a child element like any other. This cannot be inferred from the XML.

| Path | Shape |
| --- | --- |
| `/MediaContainer` | object |
| `/MediaContainer/Directory` | array |
| `/MediaContainer/Directory/Location` | array |
| `/MediaContainer/Hub` | array |
| `/MediaContainer/Hub/Metadata` | array |
| `/MediaContainer/Hub/Metadata/Collection` | array |
| `/MediaContainer/Hub/Metadata/CommonSenseMedia` | array |
| `/MediaContainer/Hub/Metadata/CommonSenseMedia/AgeRating` | array |
| `/MediaContainer/Hub/Metadata/Country` | array |
| `/MediaContainer/Hub/Metadata/Director` | array |
| `/MediaContainer/Hub/Metadata/Genre` | array |
| `/MediaContainer/Hub/Metadata/Guid` | array |
| `/MediaContainer/Hub/Metadata/Image` | array |
| `/MediaContainer/Hub/Metadata/Location` | array |
| `/MediaContainer/Hub/Metadata/Media` | array |
| `/MediaContainer/Hub/Metadata/Media/Part` | array |
| `/MediaContainer/Hub/Metadata/Media/Part/Stream` | array |
| `/MediaContainer/Hub/Metadata/Producer` | array |
| `/MediaContainer/Hub/Metadata/Rating` | array |
| `/MediaContainer/Hub/Metadata/Role` | array |
| `/MediaContainer/Hub/Metadata/UltraBlurColors` | object |
| `/MediaContainer/Hub/Metadata/Writer` | array |
| `/MediaContainer/Meta` | object |
| `/MediaContainer/Meta/FieldType` | array |
| `/MediaContainer/Meta/FieldType/Operator` | array |
| `/MediaContainer/Meta/Type` | array |
| `/MediaContainer/Meta/Type/Field` | array |
| `/MediaContainer/Meta/Type/Filter` | array |
| `/MediaContainer/Meta/Type/Sort` | array |
| `/MediaContainer/Metadata` | array |
| `/MediaContainer/Metadata/Collection` | array |
| `/MediaContainer/Metadata/Country` | array |
| `/MediaContainer/Metadata/Director` | array |
| `/MediaContainer/Metadata/Field` | array |
| `/MediaContainer/Metadata/Genre` | array |
| `/MediaContainer/Metadata/Guid` | array |
| `/MediaContainer/Metadata/Image` | array |
| `/MediaContainer/Metadata/Media` | array |
| `/MediaContainer/Metadata/Media/Part` | array |
| `/MediaContainer/Metadata/Media/Part/Stream` | array |
| `/MediaContainer/Metadata/Role` | array |
| `/MediaContainer/Metadata/UltraBlurColors` | object |
| `/MediaContainer/Metadata/Writer` | array |
| `/MediaContainer/MetadataItem` | array |
| `/MediaContainer/MetadataItem/MediaItem` | array |
| `/MediaContainer/MetadataItem/MediaItem/MediaPart` | array |
| `/MediaContainer/MetadataItem/MediaItem/MediaPart/MediaStream` | array |
| `/MediaContainer/Setting` | array |

## Scalar fields by level

### `/MediaContainer`

Observed on 17 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `allowCameraUpload` | boolean | 1/17 |
| `allowChannelAccess` | boolean | 1/17 |
| `allowMediaDeletion` | boolean | 1/17 |
| `allowSharing` | boolean | 1/17 |
| `allowSync` | boolean | 12/17 |
| `allowTuners` | boolean | 1/17 |
| `apiVersion` | string | 2/17 |
| `art` | string | 6/17 |
| `backgroundProcessing` | boolean | 1/17 |
| `certificate` | boolean | 1/17 |
| `claimed` | boolean | 1/17 |
| `companionProxy` | boolean | 1/17 |
| `content` | string | 4/17 |
| `countryCode` | string | 1/17 |
| `diagnostics` | string | 1/17 |
| `eventStream` | boolean | 1/17 |
| `friendlyName` | string | 1/17 |
| `hubSearch` | boolean | 1/17 |
| `identifier` | string | 11/17 |
| `itemClusters` | boolean | 1/17 |
| `librarySectionID` | number | 5/17 |
| `librarySectionTitle` | string | 4/17 |
| `librarySectionUUID` | string | 4/17 |
| `livetv` | number | 1/17 |
| `machineIdentifier` | string | 2/17 |
| `mediaProviders` | boolean | 1/17 |
| `mediaTagPrefix` | string | 9/17 |
| `mediaTagVersion` | number | 9/17 |
| `multiuser` | boolean | 1/17 |
| `musicAnalysis` | number | 1/17 |
| `myPlex` | boolean | 1/17 |
| `myPlexMappingState` | string | 1/17 |
| `myPlexSigninState` | string | 1/17 |
| `myPlexSubscription` | boolean | 1/17 |
| `myPlexUsername` | string | 1/17 |
| `offlineTranscode` | number | 1/17 |
| `ownerFeatures` | string | 1/17 |
| `platform` | string | 1/17 |
| `platformVersion` | string | 1/17 |
| `pluginHost` | boolean | 1/17 |
| `pushNotifications` | boolean | 1/17 |
| `readOnlyLibraries` | boolean | 1/17 |
| `size` | number | 17/17 |
| `streamingBrainABRVersion` | number | 1/17 |
| `streamingBrainVersion` | number | 1/17 |
| `sync` | boolean | 1/17 |
| `thumb` | string | 6/17 |
| `title1` | string | 7/17 |
| `title2` | string | 3/17 |
| `transcoderActiveVideoSessions` | number | 1/17 |
| `transcoderAudio` | boolean | 1/17 |
| `transcoderLyrics` | boolean | 1/17 |
| `transcoderPhoto` | boolean | 1/17 |
| `transcoderSubtitles` | boolean | 1/17 |
| `transcoderVideo` | boolean | 1/17 |
| `transcoderVideoBitrates` | string | 1/17 |
| `transcoderVideoQualities` | string | 1/17 |
| `transcoderVideoResolutions` | string | 1/17 |
| `updatedAt` | number | 1/17 |
| `updater` | boolean | 1/17 |
| `version` | string | 2/17 |
| `viewGroup` | string | 6/17 |
| `viewMode` | number | 6/17 |
| `voiceSearch` | boolean | 1/17 |

### `/MediaContainer/Directory`

Observed on 119 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `agent` | string | 10/119 |
| `allowSync` | boolean | 10/119 |
| `content` | boolean | 10/119 |
| `contentChangedAt` | number | 10/119 |
| `count` | number | 25/119 |
| `createdAt` | number | 10/119 |
| `default` | string | 1/119 |
| `defaultDirection` | string | 9/119 |
| `descKey` | string | 9/119 |
| `directory` | boolean | 10/119 |
| `filter` | string | 27/119 |
| `filterType` | string | 27/119 |
| `filters` | boolean | 10/119 |
| `firstCharacterKey` | string | 1/119 |
| `hidden` | number | 10/119 |
| `key` | string | 119/119 |
| `language` | string | 10/119 |
| `prompt` | string | 1/119 |
| `refreshing` | boolean | 10/119 |
| `scannedAt` | number | 10/119 |
| `scanner` | string | 10/119 |
| `search` | boolean | 1/119 |
| `secondary` | boolean | 12/119 |
| `size` | number | 28/119 |
| `title` | string | 119/119 |
| `type` | string | 37/119 |
| `updatedAt` | number | 10/119 |
| `uuid` | string | 10/119 |

### `/MediaContainer/Directory/Location`

Observed on 20 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `id` | number | 20/20 |
| `path` | string | 20/20 |

### `/MediaContainer/Hub`

Observed on 9 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `context` | string | 9/9 |
| `hubIdentifier` | string | 9/9 |
| `hubKey` | string | 5/9 |
| `key` | string | 9/9 |
| `more` | boolean | 9/9 |
| `promoted` | boolean | 8/9 |
| `size` | number | 9/9 |
| `style` | string | 9/9 |
| `title` | string | 9/9 |
| `type` | string | 9/9 |

### `/MediaContainer/Hub/Metadata`

Observed on 41 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `addedAt` | number | 41/41 |
| `art` | string | 41/41 |
| `audienceRating` | number | 30/41 |
| `audienceRatingImage` | string | 34/41 |
| `chapterSource` | string | 29/41 |
| `childCount` | number | 1/41 |
| `contentRating` | string | 33/41 |
| `contentRatingAge` | number | 8/41 |
| `duration` | number | 38/41 |
| `grandparentArt` | string | 18/41 |
| `grandparentGuid` | string | 18/41 |
| `grandparentKey` | string | 18/41 |
| `grandparentRatingKey` | string | 18/41 |
| `grandparentSlug` | string | 18/41 |
| `grandparentTheme` | string | 15/41 |
| `grandparentThumb` | string | 18/41 |
| `grandparentTitle` | string | 18/41 |
| `guid` | string | 41/41 |
| `hasPremiumExtras` | string | 2/41 |
| `hasPremiumPrimaryExtra` | string | 18/41 |
| `includedAt` | number | 17/41 |
| `index` | number | 22/41 |
| `key` | string | 41/41 |
| `lastViewedAt` | number | 30/41 |
| `leafCount` | number | 4/41 |
| `librarySectionID` | number | 41/41 |
| `librarySectionKey` | string | 41/41 |
| `librarySectionTitle` | string | 41/41 |
| `originalTitle` | string | 20/41 |
| `originallyAvailableAt` | string | 38/41 |
| `parentGuid` | string | 21/41 |
| `parentIndex` | number | 21/41 |
| `parentKey` | string | 21/41 |
| `parentRatingKey` | string | 21/41 |
| `parentSlug` | string | 3/41 |
| `parentStudio` | string | 3/41 |
| `parentTheme` | string | 1/41 |
| `parentThumb` | string | 21/41 |
| `parentTitle` | string | 21/41 |
| `parentYear` | number | 3/41 |
| `rating` | number | 14/41 |
| `ratingImage` | string | 18/41 |
| `ratingKey` | string | 41/41 |
| `slug` | string | 10/41 |
| `studio` | string | 17/41 |
| `summary` | string | 41/41 |
| `tagline` | string | 14/41 |
| `theme` | string | 1/41 |
| `thumb` | string | 41/41 |
| `title` | string | 41/41 |
| `titleSort` | string | 15/41 |
| `type` | string | 41/41 |
| `updatedAt` | number | 37/41 |
| `viewCount` | number | 1/41 |
| `viewOffset` | number | 14/41 |
| `viewedLeafCount` | number | 4/41 |
| `year` | number | 38/41 |

### `/MediaContainer/Hub/Metadata/Collection`

Observed on 4 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `tag` | string | 4/4 |

### `/MediaContainer/Hub/Metadata/CommonSenseMedia`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `id` | number | 1/1 |
| `oneLiner` | string | 1/1 |

### `/MediaContainer/Hub/Metadata/CommonSenseMedia/AgeRating`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `age` | number | 1/1 |
| `rating` | number | 1/1 |
| `type` | string | 1/1 |

### `/MediaContainer/Hub/Metadata/Country`

Observed on 20 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 1/20 |
| `id` | number | 1/20 |
| `tag` | string | 20/20 |

### `/MediaContainer/Hub/Metadata/Director`

Observed on 38 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 15/38 |
| `id` | number | 15/38 |
| `tag` | string | 38/38 |
| `tagKey` | string | 15/38 |
| `thumb` | string | 13/38 |

### `/MediaContainer/Hub/Metadata/Genre`

Observed on 35 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 6/35 |
| `id` | number | 6/35 |
| `tag` | string | 35/35 |

### `/MediaContainer/Hub/Metadata/Guid`

Observed on 53 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `id` | string | 53/53 |

### `/MediaContainer/Hub/Metadata/Image`

Observed on 155 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `alt` | string | 155/155 |
| `type` | string | 155/155 |
| `url` | string | 155/155 |

### `/MediaContainer/Hub/Metadata/Location`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `path` | string | 1/1 |

### `/MediaContainer/Hub/Metadata/Media`

Observed on 38 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `aspectRatio` | number | 38/38 |
| `audioChannels` | number | 38/38 |
| `audioCodec` | string | 38/38 |
| `audioProfile` | string | 16/38 |
| `bitrate` | number | 38/38 |
| `container` | string | 38/38 |
| `duration` | number | 38/38 |
| `has64bitOffsets` | boolean | 1/38 |
| `height` | number | 38/38 |
| `id` | number | 38/38 |
| `optimizedForStreaming` | number | 1/38 |
| `videoCodec` | string | 38/38 |
| `videoFrameRate` | string | 38/38 |
| `videoProfile` | string | 38/38 |
| `videoResolution` | string | 38/38 |
| `width` | number | 38/38 |

### `/MediaContainer/Hub/Metadata/Media/Part`

Observed on 38 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `audioProfile` | string | 16/38 |
| `container` | string | 38/38 |
| `duration` | number | 38/38 |
| `file` | string | 38/38 |
| `has64bitOffsets` | boolean | 1/38 |
| `hasThumbnail` | string | 2/38 |
| `id` | number | 38/38 |
| `key` | string | 38/38 |
| `optimizedForStreaming` | boolean | 1/38 |
| `size` | number | 38/38 |
| `videoProfile` | string | 38/38 |

### `/MediaContainer/Hub/Metadata/Media/Part/Stream`

Observed on 103 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `audioChannelLayout` | string | 36/103 |
| `bitDepth` | number | 17/103 |
| `bitrate` | number | 99/103 |
| `channels` | number | 36/103 |
| `chromaLocation` | string | 17/103 |
| `chromaSubsampling` | string | 17/103 |
| `codec` | string | 103/103 |
| `codedHeight` | number | 17/103 |
| `codedWidth` | number | 17/103 |
| `colorPrimaries` | string | 17/103 |
| `colorRange` | string | 17/103 |
| `colorSpace` | string | 17/103 |
| `colorTrc` | string | 17/103 |
| `default` | boolean | 45/103 |
| `displayTitle` | string | 103/103 |
| `dub` | boolean | 6/103 |
| `extendedDisplayTitle` | string | 103/103 |
| `forced` | boolean | 11/103 |
| `frameRate` | number | 17/103 |
| `hasScalingMatrix` | boolean | 6/103 |
| `hearingImpaired` | boolean | 13/103 |
| `height` | number | 17/103 |
| `id` | number | 103/103 |
| `index` | number | 103/103 |
| `language` | string | 103/103 |
| `languageCode` | string | 103/103 |
| `languageTag` | string | 103/103 |
| `level` | number | 17/103 |
| `original` | boolean | 16/103 |
| `profile` | string | 26/103 |
| `refFrames` | number | 17/103 |
| `samplingRate` | number | 36/103 |
| `scanType` | string | 6/103 |
| `selected` | boolean | 28/103 |
| `streamType` | number | 103/103 |
| `title` | string | 73/103 |
| `visualImpaired` | boolean | 2/103 |
| `width` | number | 17/103 |

### `/MediaContainer/Hub/Metadata/Producer`

Observed on 25 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 25/25 |
| `id` | number | 25/25 |
| `tag` | string | 25/25 |
| `tagKey` | string | 25/25 |
| `thumb` | string | 11/25 |

### `/MediaContainer/Hub/Metadata/Rating`

Observed on 32 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `image` | string | 32/32 |
| `type` | string | 32/32 |
| `value` | number | 32/32 |

### `/MediaContainer/Hub/Metadata/Role`

Observed on 411 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 363/411 |
| `id` | number | 363/411 |
| `role` | string | 363/411 |
| `tag` | string | 411/411 |
| `tagKey` | string | 363/411 |
| `thumb` | string | 350/411 |

### `/MediaContainer/Hub/Metadata/UltraBlurColors`

Observed on 41 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `bottomLeft` | string | 41/41 |
| `bottomRight` | string | 41/41 |
| `topLeft` | string | 41/41 |
| `topRight` | string | 41/41 |

### `/MediaContainer/Hub/Metadata/Writer`

Observed on 49 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 21/49 |
| `id` | number | 21/49 |
| `tag` | string | 49/49 |
| `tagKey` | string | 21/49 |
| `thumb` | string | 15/49 |

### `/MediaContainer/Meta/FieldType`

Observed on 8 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `type` | string | 8/8 |

### `/MediaContainer/Meta/FieldType/Operator`

Observed on 21 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `key` | string | 21/21 |
| `title` | string | 21/21 |

### `/MediaContainer/Meta/Type`

Observed on 2 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `active` | boolean | 2/2 |
| `key` | string | 2/2 |
| `title` | string | 2/2 |
| `type` | string | 2/2 |

### `/MediaContainer/Meta/Type/Field`

Observed on 39 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `key` | string | 39/39 |
| `subType` | string | 14/39 |
| `title` | string | 39/39 |
| `type` | string | 39/39 |

### `/MediaContainer/Meta/Type/Filter`

Observed on 27 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 27/27 |
| `filterType` | string | 27/27 |
| `key` | string | 27/27 |
| `title` | string | 27/27 |
| `type` | string | 27/27 |

### `/MediaContainer/Meta/Type/Sort`

Observed on 9 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `active` | boolean | 1/9 |
| `activeDirection` | string | 1/9 |
| `default` | string | 1/9 |
| `defaultDirection` | string | 9/9 |
| `descKey` | string | 9/9 |
| `firstCharacterKey` | string | 1/9 |
| `key` | string | 9/9 |
| `title` | string | 9/9 |

### `/MediaContainer/Metadata`

Observed on 19335 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `addedAt` | number | 19335/19335 |
| `art` | string | 19229/19335 |
| `audienceRating` | number | 11950/19335 |
| `audienceRatingImage` | string | 15324/19335 |
| `chapterSource` | string | 11619/19335 |
| `contentRating` | string | 11386/19335 |
| `contentRatingAge` | number | 1620/19335 |
| `createdAtAccuracy` | string | 12/19335 |
| `createdAtTZOffset` | string | 12/19335 |
| `duration` | number | 19335/19335 |
| `guid` | string | 19335/19335 |
| `hasPremiumExtras` | string | 390/19335 |
| `hasPremiumPrimaryExtra` | string | 13118/19335 |
| `key` | string | 19335/19335 |
| `lastRatedAt` | number | 2/19335 |
| `lastViewedAt` | number | 520/19335 |
| `librarySectionID` | number | 1/19335 |
| `librarySectionKey` | string | 1/19335 |
| `librarySectionTitle` | string | 1/19335 |
| `loudnessAnalysisVersion` | string | 38/19335 |
| `originalTitle` | string | 9914/19335 |
| `originallyAvailableAt` | string | 19311/19335 |
| `rating` | number | 8124/19335 |
| `ratingImage` | string | 13884/19335 |
| `ratingKey` | string | 19335/19335 |
| `skipCount` | number | 154/19335 |
| `slug` | string | 5380/19335 |
| `studio` | string | 12473/19335 |
| `summary` | string | 19335/19335 |
| `tagline` | string | 9214/19335 |
| `thumb` | string | 19335/19335 |
| `title` | string | 19335/19335 |
| `titleSort` | string | 6075/19335 |
| `type` | string | 19335/19335 |
| `updatedAt` | number | 19333/19335 |
| `userRating` | number | 4/19335 |
| `viewCount` | number | 284/19335 |
| `viewOffset` | number | 244/19335 |
| `year` | number | 19331/19335 |

### `/MediaContainer/Metadata/Collection`

Observed on 1992 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `tag` | string | 1992/1992 |

### `/MediaContainer/Metadata/Country`

Observed on 16229 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 1/16229 |
| `id` | number | 1/16229 |
| `tag` | string | 16229/16229 |

### `/MediaContainer/Metadata/Director`

Observed on 13686 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `tag` | string | 13686/13686 |

### `/MediaContainer/Metadata/Field`

Observed on 2 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `locked` | boolean | 2/2 |
| `name` | string | 2/2 |

### `/MediaContainer/Metadata/Genre`

Observed on 23827 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `filter` | string | 1/23827 |
| `id` | number | 1/23827 |
| `tag` | string | 23827/23827 |

### `/MediaContainer/Metadata/Guid`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `id` | string | 1/1 |

### `/MediaContainer/Metadata/Image`

Observed on 41936 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `alt` | string | 41936/41936 |
| `type` | string | 41936/41936 |
| `url` | string | 41936/41936 |

### `/MediaContainer/Metadata/Media`

Observed on 19341 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `aspectRatio` | number | 19335/19341 |
| `audioChannels` | number | 19335/19341 |
| `audioCodec` | string | 19335/19341 |
| `audioProfile` | string | 7245/19341 |
| `bitrate` | number | 19337/19341 |
| `container` | string | 19337/19341 |
| `deletedAt` | number | 4/19341 |
| `duration` | number | 19337/19341 |
| `has64bitOffsets` | boolean | 220/19341 |
| `height` | number | 19335/19341 |
| `id` | number | 19341/19341 |
| `optimizedForStreaming` | number | 220/19341 |
| `videoCodec` | string | 19335/19341 |
| `videoFrameRate` | string | 19193/19341 |
| `videoProfile` | string | 19265/19341 |
| `videoResolution` | string | 19335/19341 |
| `width` | number | 19335/19341 |

### `/MediaContainer/Metadata/Media/Part`

Observed on 19349 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `audioProfile` | string | 7253/19349 |
| `container` | string | 19345/19349 |
| `duration` | number | 19343/19349 |
| `file` | string | 19349/19349 |
| `has64bitOffsets` | boolean | 220/19349 |
| `hasChapterTextStream` | boolean | 2/19349 |
| `hasThumbnail` | string | 1040/19349 |
| `id` | number | 19349/19349 |
| `key` | string | 19349/19349 |
| `optimizedForStreaming` | boolean | 220/19349 |
| `packetLength` | number | 34/19349 |
| `size` | number | 19349/19349 |
| `timeStamp` | boolean | 4/19349 |
| `videoProfile` | string | 19273/19349 |

### `/MediaContainer/Metadata/Media/Part/Stream`

Observed on 4 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `audioChannelLayout` | string | 1/4 |
| `bitDepth` | number | 2/4 |
| `bitrate` | number | 4/4 |
| `channels` | number | 1/4 |
| `chromaLocation` | string | 1/4 |
| `chromaSubsampling` | string | 1/4 |
| `codec` | string | 4/4 |
| `codedHeight` | number | 1/4 |
| `codedWidth` | number | 1/4 |
| `colorPrimaries` | string | 1/4 |
| `colorRange` | string | 1/4 |
| `colorSpace` | string | 1/4 |
| `colorTrc` | string | 1/4 |
| `default` | boolean | 2/4 |
| `displayTitle` | string | 4/4 |
| `extendedDisplayTitle` | string | 4/4 |
| `frameRate` | number | 1/4 |
| `hasScalingMatrix` | boolean | 1/4 |
| `height` | number | 1/4 |
| `id` | number | 4/4 |
| `index` | number | 4/4 |
| `language` | string | 4/4 |
| `languageCode` | string | 4/4 |
| `languageTag` | string | 4/4 |
| `level` | number | 1/4 |
| `profile` | string | 2/4 |
| `refFrames` | number | 1/4 |
| `samplingRate` | number | 1/4 |
| `scanType` | string | 1/4 |
| `selected` | boolean | 1/4 |
| `streamType` | number | 4/4 |
| `title` | string | 3/4 |
| `width` | number | 1/4 |

### `/MediaContainer/Metadata/Role`

Observed on 37420 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `tag` | string | 37420/37420 |

### `/MediaContainer/Metadata/UltraBlurColors`

Observed on 19281 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `bottomLeft` | string | 19281/19281 |
| `bottomRight` | string | 19281/19281 |
| `topLeft` | string | 19281/19281 |
| `topRight` | string | 19281/19281 |

### `/MediaContainer/Metadata/Writer`

Observed on 19740 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `tag` | string | 19740/19740 |

### `/MediaContainer/MetadataItem`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `addedAt` | number | 1/1 |
| `guid` | string | 1/1 |
| `id` | number | 1/1 |
| `index` | number | 1/1 |
| `instanceRatingKey` | string | 1/1 |
| `originallyAvailableAt` | string | 1/1 |
| `parentTitle` | string | 1/1 |
| `refreshedAt` | number | 1/1 |
| `title` | string | 1/1 |

### `/MediaContainer/MetadataItem/MediaItem`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `id` | number | 1/1 |

### `/MediaContainer/MetadataItem/MediaItem/MediaPart`

Observed on 1 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `duration` | number | 1/1 |
| `file` | string | 1/1 |
| `hash` | string | 1/1 |
| `id` | number | 1/1 |
| `openSubtitleHash` | string | 1/1 |
| `size` | number | 1/1 |

### `/MediaContainer/MetadataItem/MediaItem/MediaPart/MediaStream`

Observed on 4 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `audioChannelLayout` | string | 1/4 |
| `bitDepth` | string | 2/4 |
| `channels` | number | 1/4 |
| `chromaLocation` | string | 1/4 |
| `chromaSubsampling` | string | 1/4 |
| `codec` | string | 4/4 |
| `codedHeight` | string | 1/4 |
| `codedWidth` | string | 1/4 |
| `colorPrimaries` | string | 1/4 |
| `colorRange` | string | 1/4 |
| `colorSpace` | string | 1/4 |
| `colorTrc` | string | 1/4 |
| `frameRate` | string | 1/4 |
| `hasScalingMatrix` | string | 1/4 |
| `height` | string | 1/4 |
| `id` | number | 4/4 |
| `index` | number | 4/4 |
| `language` | string | 4/4 |
| `level` | string | 1/4 |
| `profile` | string | 2/4 |
| `refFrames` | string | 1/4 |
| `requiredBandwidths` | string | 4/4 |
| `samplingRate` | string | 1/4 |
| `scanType` | string | 1/4 |
| `title` | string | 3/4 |
| `type` | number | 4/4 |
| `width` | string | 1/4 |

### `/MediaContainer/Setting`

Observed on 162 node(s).

| Field | JSON type | Present |
| --- | --- | --: |
| `advanced` | boolean | 162/162 |
| `default` | boolean, number, number, string | 162/162 |
| `enumValues` | string | 19/162 |
| `group` | string | 162/162 |
| `hidden` | boolean | 162/162 |
| `id` | string | 162/162 |
| `label` | string | 162/162 |
| `summary` | string | 162/162 |
| `type` | string | 162/162 |
| `value` | boolean, number, number, string | 162/162 |
