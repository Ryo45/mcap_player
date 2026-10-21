# Camera formats fixture

生成コマンド（出力先が存在しない状態で実行）:

```sh
cargo run -p ros-fixture -- camera-formats tests/fixtures/camera-formats/camera_formats.mcap
```

7 cameras × 30 frames、10 Hz、320×240。topicの末尾はrgb8/bgr8/rgba8/bgra8/mono8/png/jpeg。
log timeとheader stampは1735689600000000000 + sequence×100000000 ns。
Rawは各行3 bytesのpadding（222）を持ち、is_bigendianは0/1交互。全画面のRGBは[20+sequence×3,80,160]。
mono8は赤成分を輝度とし、RGBA/BGRAのalphaは255。PNGはlossless、JPEGはquality95で許容誤差3。
シンプルな既知色により、縮小filterと独立して色順・padding・Seek後のframeを検証する。
位置によって異なる画素のsamplingや非不透明alphaはrendererの単体テストを併用する。

SHA-256: `a40db5239e51a2cb35ee55975fb169dc3e3337f968c421644be160e038fa1312`

## Transparent Camera fixture

```sh
cargo run -p ros-fixture -- camera-alpha tests/fixtures/camera-formats/camera_alpha.mcap
```

RGBA8 / BGRA8 / PNGの3 camera × 30 frames。寸法・時刻・RGB・Raw行paddingは上と同じ。
alphaはsequenceごとに128→0→255を繰り返す。旧frameの残留、繰り返すCanvas合成でのalpha蓄積を検査する。
SHA-256: `9c803c48aa1792628ea9f698c3336db3599206b7bd318c740813509e02ffc6b3`
