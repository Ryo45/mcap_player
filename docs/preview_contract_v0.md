# Preview and Bookmark platform-neutral contract v0

## 1. 目的

Previewはログ全体を低コストで探索するための粗いデータ経路であり、現在時刻の正確な状態を構築するPush pathと、数値信号を抽出するPlot query pathから分離する。

```text
Push
  Recording playback → feature controllers → Exact state at playback cursor

Query
  PlotLoader → LoadedSignal → full-resolution plot query

Preview
  future preview sidecar → PreviewSnapshot → coarse scrub display
```

このv0はプラットフォーム中立な型、Serde表現、validationだけを定義する。reader、writer、worker、UI、sidecar MCAPの具体的なrecord mappingは含まない。

## 2. Schema version

Bookmark documentとPreview build informationは独立してversion管理する。

```rust
CURRENT_BOOKMARK_SCHEMA_VERSION = 1
CURRENT_PREVIEW_SCHEMA_VERSION = 1
```

未知fieldはSerdeの通常動作として無視できる。未知のschema versionは`BookmarkDocument`または`PreviewBuildInfo`のdeserialize時にerrorとなり、黙って受理しない。

`PreviewSnapshot`自身にはversionを重複して持たせず、将来のsidecar全体に対応する`PreviewBuildInfo`がPreview schema versionを保持する。

## 3. 時刻とfidelity

- `ArrivalTime`と`MeasurementTime`は既存どおりsigned 64-bit nanosecondsである。
- JSONでも浮動小数点秒へ変換せず整数としてserializeする。
- `DataFidelity::Preview`は粗い探索用、`DataFidelity::Exact`は元ログ相当の正確さを表す。
- signalは`Envelope { bucket_ns }`または`Exact`として個別にfidelityを持つ。
- `TimeRange`の両端はinclusiveな契約とし、`start <= end`を要求する。

## 4. Preview contract

`PreviewRequest`はrange、任意のtarget time、camera/signal選択、結果数budgetを表す。I/Oやrequest generationは含まない。

`PreviewSnapshot`は次を独立したoptional collectionとして保持する。

- `CameraPreviewFrame`: 現在はJPEGだけ。Camera ID、measurement/arrival time、frame ID、画像寸法、bytesを保持する。
- `SignalOverview`: signalごとのfidelityと時間順のmin/max envelope bucket。
- `TimedPosition2`: arrival time付き2D trajectory点。

すべてのcollectionは空でよい。signalだけ、cameraだけ、trajectoryだけの部分snapshotも有効である。

Preview JSON例：

```json
{
  "fidelity": "preview",
  "availableRange": {
    "start": 1785591563485080407,
    "end": 1785592463224014194
  },
  "cameraFrames": [
    {
      "cameraId": 0,
      "measurementTime": 1785592013000000000,
      "arrivalTime": 1785592013010000000,
      "frameId": "camera_front_optical",
      "encoding": "jpeg",
      "width": 640,
      "height": 360,
      "bytes": [255, 216, 255, 217]
    }
  ],
  "signalOverviews": [
    {
      "signalId": "speed",
      "fidelity": {
        "envelope": {
          "bucketNs": 100000000
        }
      },
      "buckets": [
        {
          "startTime": 1785592013000000000,
          "endTime": 1785592013100000000,
          "first": 0.8,
          "last": 1.1,
          "min": 0.7,
          "max": 1.2,
          "count": 10
        }
      ]
    }
  ],
  "trajectory": [
    {
      "time": 1785592013000000000,
      "position": [1.0, 2.0]
    }
  ]
}
```

## 5. SignalBucket validation and merge

各bucketは次を満たす。

- `start_time <= end_time`
- `count > 0`
- `first`、`last`、`min`、`max`がfinite
- `min <= first <= max`
- `min <= last <= max`

bucket列は前bucketの`end_time <=`次bucketの`start_time`となる時間順を要求する。

`merge_signal_buckets()`は空でない時間順bucket列を一つへ集約する。

```text
start_time = first bucket.start_time
end_time   = last bucket.end_time
first      = first bucket.first
last       = last bucket.last
min        = minimum of all bucket.min
max        = maximum of all bucket.max
count      = checked addition of all bucket.count
```

空入力、順序不正、不正bucket、`u32` count overflowはerrorとする。countはsaturateさせない。

## 6. Camera and trajectory validation

Camera previewはwidth/heightがともに非0で、bytesが空でないことを要求する。JPEG bitstream自体のdecode validationはこの契約層では行わない。

trajectoryの各座標はfiniteで、点列の時刻は非減少でなければならない。同一時刻の複数点は許可する。

## 7. Bookmark contract

Bookmarkはpointまたはintervalを表す。

- IDは空または空白だけにできない。
- labelは空白だけにできない。
- intervalの`end_time`は`time`以上でなければならない。
- 一つのdocument内でBookmark IDは一意でなければならない。
- Source fingerprintのalgorithm/valueは空または空白だけにできない。

Bookmark JSON例：

```json
{
  "schemaVersion": 1,
  "source": {
    "algorithm": "sha256",
    "value": "0123456789abcdef"
  },
  "bookmarks": [
    {
      "id": "obstacle-1",
      "time": 1785592013000000000,
      "endTime": null,
      "label": "Obstacle",
      "note": "Inspect front camera"
    },
    {
      "id": "turn-1",
      "time": 1785592020000000000,
      "endTime": 1785592025000000000,
      "label": "Turn",
      "note": null
    }
  ]
}
```

Preview build information例：

```json
{
  "schemaVersion": 1,
  "generatorName": "mcap-viewer",
  "generatorVersion": "0.1.0",
  "source": {
    "algorithm": "sha256",
    "value": "0123456789abcdef"
  }
}
```

## 8. 所有関係と既存経路との境界

- Exact feature controllersはPreviewを所有しない。
- `LoadedSignal`は変更せず、Signal Overviewと共有しない。
- `ViewerSession`はPreview APIを持たない。
- `PlotLoader`をPreview loaderへ一般化しない。
- `ViewerInteractionState.preview_time`は今回変更しない。
- Range Read Spikeをproduction readerへ移動しない。
- Preview/Bookmark契約はegui、wgpu、web-sys、File APIへ依存しない。

## 9. preview.mcap v0で未決定のmapping

次の内容はreader/writer実装時に決定する。

- MCAP profile/library文字列
- `PreviewBuildInfo`をMetadata、Attachment、または専用Messageのどれで保存するか
- Camera preview、Signal bucket、Trajectoryのtopic名とschema encoding
- 1 Messageに含める時間範囲とchunk境界
- camera frameの選択規則とJPEG再encode条件
- signal bucket幅とbudgetからの生成規則
- trajectoryの座標frameとsource topic
- source fingerprint algorithmの初期標準
- sidecar filename、元MCAPとの関連付け、atomic replacement
- sidecar内のindex／compression設定

これらを確定する前に、Preview contractを通常再生のPush pathまたはPlot query pathへ統合しない。
