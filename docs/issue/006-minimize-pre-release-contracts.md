# Remote catalogをrecording factsへ限定する

- Priority: P2
- 規模: S-M
- 状態: 完了

## 背景・課題

実装前のRecording Serverは`StreamSemantic::Camera`を配信していましたが、Web clientはsemanticを
使わず`schema_name`からCameraを再判定していました。`capabilities`と`representation`にも実際の
client分岐に使われない値があり、storage serverとviewerの両方がfeature meaningを持つ二重truthでした。

## 解決案

- catalogをid/topic/schema name/schema encoding/message encoding/count/time/revisionというrecording factsへ寄せる。
- clientが未使用のsemantic/capabilitiesをrelease前に削除する。
- representationが常にros2-cdrならprotocol invariantへ上げ、fieldを削除する。
- timestamp decimal string、revision、batch framing、exclusive range semanticsは実利用契約として残す。

Layout policyは[010](010-layout-internal-contract.md)へ分離します。

## 完了条件

- Recording ServerがCamera/Panelなどviewer featureを分類しない。
- wire fieldはclientが検証・分岐に使うrecording factだけになる。

## 実装結果

- wire `StreamDescriptor`からviewer feature meaningの`semantic`と固定値`representation`を削除した。
- clientが参照しないcatalog `capabilities`も削除し、continuationの実契約はbatch response headerと
  pagination validationに残した。
- Serverはid/topic/schema name/schema encoding/message encoding/message countだけを配信する。
- Webは`message_encoding == "cdr"`を検証し、Camera/Odometry/TFの意味は`schema_name`から一度だけ判定する。
- timestamp decimal string、recording revision、exclusive range、Batch framingは維持した。
