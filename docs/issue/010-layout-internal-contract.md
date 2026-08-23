# Layoutを0.1のbundled internal presetとして明示する

- Priority: Optional / release policy decision
- 規模: S
- 状態: 方針決定・完了

## 背景・課題

Layout JSONはversion、unknown panel preservation、save APIを持ちますが、0.1で利用者が指定できるのは
include_strされたStandard/Showcaseだけです。外部supported formatと誤認される余地があります。

## 方針

0.1ではbundled internal presetでありbackward compatibilityを保証しないとREADMEへ明記します。
configVersion/placeholder preservationが実装を阻害する場合だけ削除し、単なるcrate移動や大規模simplificationは
行いません。将来外部formatとして公開する時にload/save/migration policyを定義します。

## 完了条件

- 0.1で外部互換を約束しないことが文書化される。
- unused compatibility codeがP0/P1 implementationを妨げない。

## 実装結果

- READMEにStandard/Showcaseが0.1のbundled internal presetであり、外部互換を保証しないと明記した。
- `schemaVersion` / `configVersion`とplaceholderは、同梱presetの開発時検証とvisible errorに実利用して
  いるため維持した。migration/compatibility facadeとしては扱わない。
- 一variantだったCamera `fit` field/enumはpreset JSONと実装から削除し、unknown fieldをconfig errorとして
  可視化する。
- external Layout load/save/migration policyは公開時まで導入しない。
