# ランダムテスト・クロスチェック機能 要件定義

## 概要

`cargo compete test` / `cargo compete submit` コマンドに、ランダムテストおよびクロスチェック機能を追加する。

---

## コマンドインタフェース

### オプションの意味

- `--no-sample` はサンプルテストのみを省略する。random test / cross-check は実行する。
- `--no-test` は提出前のテスト全体を省略する。`cargo compete submit` 単独でのみ使用できる。

### `cargo compete test`

```bash
# ランダムテスト（サンプル通過後にN件実行、省略時はデフォルト5件）
cargo compete test a --random
cargo compete test a --random 50

# クロスチェック（サンプル通過後に別実装と比較、省略時はデフォルト100件）
cargo compete test a --cross src/bin/a_brute.rs
cargo compete test a --cross "a copy.rs" 50

# サンプルテストを省略してランダムテスト/クロスチェックのみ実行
cargo compete test a --random --no-sample
cargo compete test a --cross "a_brute.rs" --no-sample
```

### `cargo compete submit`

```bash
# 通常の提出前サンプルテストを行って提出
cargo compete submit a

# 提出前テストを全て省略して提出
cargo compete submit a --no-test

# サブミット前にランダムテストを実行（失敗したらサブミットしない）
cargo compete submit a --random 50

# サンプルテストを省略してランダムテストのみ実行し、全ACで提出
cargo compete submit a --random --no-sample

# サブミット前にクロスチェックを実行（全て一致したら提出）
cargo compete submit a --cross "a_brute.rs"

# サンプルテストを省略してクロスチェックのみ実行し、全て一致したら提出
cargo compete submit a --cross "a_brute.rs" --no-sample
```

**無効な組み合わせ（エラー終了）:**

| コマンド | 理由 |
|---|---|
| `cargo compete test a --no-test` | `test` でテスト全体を省略する意味がない |
| `cargo compete submit a --random --no-test` | random test を指定しながらテスト全体を省略する指定は矛盾する |
| `cargo compete submit a --cross "a_brute.rs" --no-test` | cross-check を指定しながらテスト全体を省略する指定は矛盾する |
| `cargo compete test a --no-sample` | 省略後に実行する random test / cross-check がない |
| `cargo compete submit a --no-sample` | 省略後に実行する random test / cross-check がない |

---

## 機能1: ランダムテスト（`--random`）

### 実行フロー

1. サンプルテストを実行（`--no-sample` なら省略）
2. サンプルが全て通過したらランダムテストを実行
3. 1件でも RE/TLE が出たら非ゼロ終了（サブミットに進まない）

#### snowchains を使ったランダムテスト実行フロー詳細

1. 全テストケースの入力を生成（`generate_random_input`）
2. 各テストケースを `BatchTestCase { out: None, ... }` で構築
   - `out: None` → `DeterministicExpectedOutput::Pass` → 正常終了なら常に Accepted、RE/TLE のみ失敗
3. `judge()` に全ケースを**一括**投入（進捗バー表示あり）
4. `print_pretty` で全ケース表示（`expected:` 行は出ない、`stderr:` は空でない時のみ出る）

### 制約情報の取得

- 制約は問題取得時（`cargo compete retrieve testcases` / `cargo compete new`）に
  `task.html` から抽出され、各 `testcases/{problem}.yml` の `random_test:` セクションに
  永続化される
- ランダムテスト実行時は **yml をそのまま読み込む**（HTML 再パースは不要）
- AtCoder 専用（他プラットフォームでは `random_test:` セクションが生成されないためスキップ）

### `ordering` / `not_equal` の仕様

- `ordering` は整数変数間のみ yml に出力する。チェーン制約は推移閉包を取り、`l <= r <= n` なら `l <= r`, `r <= n`, `l <= n` を保存する。
- yml に保存する `ordering` の確定後、既知の range を同じ大小関係に沿って固定点伝播し、欠落 range を補完する。たとえば `k <= n`, `k <= m` と `n,m <= 200000` が保存される場合は `k.range.hi = 200000` を記載する。
- `K <= Σ C_i L_i` のように集約式を含む比較は単純な `ordering` に近似せず、未対応制約として `skipped` に保持する。
- ランダム生成では `ordering` を scalar / 通常配列 / rows の各要素に適用する。配列同士は出力順に flatten した同じ index 同士の比較とし、親スコープの既決定 scalar による range narrowing も行う。
- 通常の二次元以上の数値配列は row-major / 出力順に flatten して比較する。たとえば同形の `A` と `B` なら `A[i][j] <= B[i][j]` と同等に扱う。jagged array の配列間 `ordering` は yml 生成対象外とする。
- `not_equal` は整数同士、および Chars scalar 同士のみ yml に出力する。
- `not_equal` の抽出結果だけを理由に `vars` を新規作成しない。後続の range 制約または入力 `format` に対象変数が存在する場合のみ yml に記載するため、`i != j` を前提とする `(X_i, Y_i) != (X_j, Y_j)` の `i`, `j` は `vars` や `not_equal` に出力しない（abc448_f）。
- 整数 scalar 同士は値が異なることを確認する。整数 scalar と通常配列 / rows column では、配列の全要素が scalar と異なることを確認する。整数配列同士 / rows column 同士では、出力順に flatten した同じ index 同士が異なることを確認する。
- Chars 同士の `not_equal` は Chars scalar のみ対象とし、文字列全体が異なることを確認する。Chars array と jagged array の配列間 `not_equal` は対象外とする。
- ランダム生成では、配列の最後にまとめて棄却するのではなく、各要素生成時に既決定値を禁止値として除外する。
- abc455_f と abc455_g で、ordering の推移・配列・スコープまたぎ制約の再検証済み。

### 制約パース対応範囲

| 記法 | 対応状況 |
|------|---------|
| `1 \leq N \leq 10^5` | ✅ 実装済み |
| `3\times 10^5` などの式 | ✅ 実装済み |
| `1 \leq A,B \leq N`（複数変数） | ✅ 実装済み |
| `1 \leq M \leq N \leq 10`（チェーン） | ✅ 実装済み |
| `A_i \leq N`（変数依存上限） | ✅ 実装済み |
| `N-1`、`N+1` などのオフセット | ✅ 実装済み |
| `\dfrac`、`\frac`、`\sqrt` を含む式 | ✅ スキップする |
| `\min(N, 10^5)` / `\max(A, B)` などの関数 | ⚠️ 関数値を bound として評価しない。`min` / `max` 自体は変数名にせず、内部の入力変数は通常の抽出対象とする |
| `T \leq \sum N_i`（sum constraint） | ✅ 実装済み |

パースできなかった制約はスキップし、末尾に警告として表示する。
> **方針:** abcでよくある制約についてなるべく対応する

### コーナーケース生成戦略

件数配分: Random戦略を1件（count<10）または2件（count≥10）割り当て、残りはシャッフルしたコーナーケース戦略全種類を**ランダム(重複なし)に**割り当てる。全種類カバー後はコーナー30%・ランダム70%の混合とする。ただし全種類カバー後の30%コーナーは**ランダム要素を持つ戦略のみ**から選択する（ランダム要素のない戦略は重複しても出力が同一になるため）。
※サイズが小さい入力は実用上確認しやすいので、敢えてSmallSize(k)のみ3件に対応するようにしている

**戦略は1テストケースにつき1つ選ばれ、入力全体に適用される。** ただし効果は変数・入力の種類によって異なる。

凡例（`all_distinct配列` / `配列間制約あり配列` 列）: `×` = その戦略を対象配列の
*要素* には適用しない。`all_distinct配列` では素の相異な順列で生成し、`配列間制約あり配列` では通常の Random 生成に `ordering` の range narrowing と `not_equal` の禁止値除外を適用して生成する。ただし**同じ戦略はスカラー／対象外の配列には通常どおり適用**し、ケース自体はスキップしない。
空欄 = 通常どおり戦略の形状を適用。

| 戦略 | スカラー整数 | 整数配列要素 | 文字列要素| 二次元以上配列の扱い | all_distinct配列 | 配列間制約あり配列 | デフォルト | all_distinctあり配列あり | 配列あり、かつ、all_distinctまたは配列間制約なし | ランダム要素 | 検出する問題パターン |
|------|-----------|------------|--------------|-------------------|------------|------------------|---------|------------------|------------------|------------|-------------------|
| AllMax | 上限値 | 全要素=上限値 | 末尾文字のみ（例: `zzzzz`） | 各行に左記を適用 | × |   | ⭕️ |   |   | — | 大入力でのTLE・オーバーフロー |
| AllMin | 下限値 | 全要素=下限値 | 先頭文字のみ（例: `a`） | 各行に左記を適用 | × |   | ⭕️ |   |   | — | 0・1要素の境界処理 |
| SmallSize(k) ※k=1,2,3の3件 | サイズ変数(テストケース数含む)=k.clamp(lo,hi)、他ランダム | ランダム | ランダム | 左記に影響を受ける |   |   | ⭕️ |   |   | ⭕️ | 小さい配列での動作 |
| ZeroCorner | 0 が範囲に入る変数は 0、入らない変数はランダム | 0 が範囲に入る要素は 0、入らない要素はランダム | 0 が範囲に入る要素は 0、入らない要素はランダム | 左記に影響を受ける | × | × | ⭕️ |   |   | ⭕️ | 符号変化・ゼロ除算・0境界 |
| MaxSize | サイズ変数を最大値・非サイズ変数はランダム。**sum_limit 制約がある場合は T=1（テストケース数を 1）** | ランダム | ランダム | 左記に影響を受ける |   |   | ⭕️ |   |   | ⭕️ | sum 制約下で各サイズ変数が最大となるケース・配列の最大規模 |
| ArrayMonoInc | ランダム | 単調増加 | charset 内で増加（先頭→末尾文字） | 各行独立に左記を適用 |   | × | — | ⚪︎ | ⭕️ | ⭕️ | ソート済み入力・二分探索の境界 |
| ArrayMonoDec | ランダム | 単調減少 | charset 内で減少（末尾→先頭文字） | 各行独立に左記を適用 |   | × | — | ⚪︎ | ⭕️ | ⭕️ | 逆順ソート済み入力 |
| ArrayAllSame | ランダム | 全要素=同一ランダム値・全行同一 | 全文字列=同一ランダム1文字を繰り返し | 各行独立に左記を適用 | × | × | — |   | ⭕️ | ⭕️ | 全同値・重複処理 |
| ArrayAltMaxMin | ランダム | 上限・下限を交互（最初の値はランダム） | charset の末尾文字・先頭文字を交互（最初の値はランダム） | 市松模様（末尾文字・先頭文字を行列インデックスで交互、初期値はランダム）（例: `#.#`/`.#.`/`#.#`）、整数配列や3次元配列も準ずる | × | × | — |   | ⭕️ | ⭕️ | 交互パターン・奇偶インデックス処理 |
| ArrayMountain | ランダム | 増加→減少（山型） | charset 内で増加→減少（山型） | 各行独立に左記を適用 |   | × | — | ⚪︎ | ⭕️ | ⭕️ | 単峰性を仮定したアルゴリズム |
| ArrayOneMaxRestMin | ランダム | ランダムな1要素=上限、残り=下限 | ランダムな1文字列=末尾文字のみ、残り=先頭文字のみ | 各行独立に左記を適用 | × | × | — |   | ⭕️ | ⭕️ | 外れ値・孤立した最大値 |
| ArrayNarrowRange | ランダム | 連続する2値（ランダム位置）のみを各要素に使用 | ランダム長・連続する2文字のみを各文字に使用 | 各行独立に左記を適用 | × | × | — |   | ⭕️ | ⭕️ | 値域が狭い場合のバグ・境界付近の挙動 |
| ArrayPeriodic ※1件 | ランダム | 2〜5要素（ランダム）の周期パターンを繰り返す | ランダム長・2〜5文字（ランダム）の周期パターンを繰り返す | 各行独立に左記を適用 | × | × | — |   | ⭕️ | ⭕️ | 周期性を仮定・無視したアルゴリズム |
| Random | ランダム | ランダム | ランダム長・ランダム文字 | ランダム文字 |   |   | ⭕️ |   |   | ⭕️ | 一般ケース |

※スカラー整数はサイズ変数を含む

- ここでいう配列間制約は、通常配列（二次元以上を含む）または rows column 同士の `ordering` / `not_equal` を指す。jagged array は含めない。
- scalar-array 制約（例: `x <= a_i`, `x != a_i`）は配列間制約には含めない。Array 系戦略は維持し、各要素生成時に scalar との `ordering` による range narrowing と `not_equal` の禁止値除外を適用する。

#### sum_limit 制約の横断的扱い

`vars[v].sum_limit = L` が指定されている場合（例: `\sum N_i \leq 2 \times 10^5`）、
文字列 `S` の「長さの総和」制約は Chars 本体ではなく、その生成長 domain `vars[|s|].sum_limit` として保存する。
**MaxSize 以外のすべての戦略**でも以下のルールを適用する:

- 該当の sum 対象変数（例: `n`）の **動的上限を `L / T` に設定**する（`T` は同入力内のテストケース数）
- ランダム整数生成時はこの動的上限を本来の上限の代わりに使う
- これにより、`T` が大きいと各 `n` の上限が小さくなり、合計が `L` を超えないように自動調整される

MaxSize 戦略のみは例外で、`T = 1` にして sum_limit を 1 ケースに集中させる
（残りのサイズ変数は本来の上限値を採用）。

### 二次元以上配列の横断的扱い

ymlファイルでlenとなっている部分(Charsならvar定義側)に対して戦略を適用する (※lenがない場合はない想定)
→つまり、各行に対して戦略を適用する。ArrayOneMaxRestMinなら各行独立にランダムな位置を上限にする

数値の場合と文字列の場合の扱いの差は整数配列、文字列参照

例外的にArrayAltMaxMinは各行依存で市松模様にする。整数の場合も。可変長二次元配列(Jagged)は各行を独立に生成するため行内の交互のみ保証する（行をまたいだ先頭の交互は保証しない）。3次元なら最終的な出力が市松模様に見えるように(つまり問題文での縦方向=2次元目*3次元目を1次元として扱って市松模様)

### 出力フォーマット（ランダムテスト）

#### AC
```
(サンプルチェックの最終行)

══════════════════════════════════════════
               random tests
══════════════════════════════════════════
1/5 ("corner1") Accepted (12 ms)                           ←progress bar
2/5 ("corner2") Runtime Error (exit status: 1) (3 ms)

1/5 ("corner1") Accepted (12 ms)                           ←print_pretty
stdin:
{input}
actual:
{output}

2/5 ("corner2") Runtime Error (exit status: 1) (3 ms)
stdin:
{input}
actual:
EMPTY
stderr:

note: output beyond --display-limit (default: 4KiB; e.g. 152834 B) is truncated; change the limit with --display-limit
note: Accepted means no crash or TLE; output correctness is not verified ← ACがある場合のみ
warning: skipped N unsupported constraint(s): {制約内容}  ← スキップがある場合のみ
error: {失敗件数}/{総件数} tests failed  ← 失敗がある場合のみ

```

**注記:**
- `{name}` は コーナーケースなら `corner1`, `corner2`, ...、ランダムケースなら `random1`, `random2`, ... の形式
- Accepted はクラッシュ・TLE なしを意味し、出力の正しさは検証しない
- 各テストケースについて `print_pretty` の出力をそのまま使う（上記フォーマットはそこを指定するものではない）
- スキップした制約の警告は**末尾のみ**出力する（`judge()` + `out:None` ベースに変更後）
- `note` / `warning` / `error` は一貫した色付けで自前出力する（クロスチェック側も同様）
- バナー前後の空行有無に注意する（クロスチェック側も同様）

---

## 機能2: クロスチェック（`--cross`）

### 実行フロー

1. メインバイナリのサンプルテスト（`--no-sample` なら省略）✅ 実装済み
2. クロスバイナリを `Cargo.toml` に自動登録（未登録の場合）
3. クロスバイナリをビルド
4. クロスバイナリのサンプルテスト（`--no-sample` なら省略）✅ 実装済み
   - 愚直解は低速なことが多いため、**制限時間なし**で実行する
5. ランダム入力をクロスバイナリに流して期待出力を収集（RE/TLE のケースはスキップ）
6. 期待出力に対してメインバイナリを判定
7. 1件でも WA/RE/TLE が出たら非ゼロ終了

#### snowchains を使ったクロスチェック実行フロー詳細

1. 全テストケースの入力を生成（`generate_random_input`）
2. クロスバイナリに `run_with_input` で実行 → `Ok(output)` のみ採用（RE/TLE はスキップ）
3. 採用ケースを `BatchTestCase { out: Some(brute_output), ... }` で構築　※brute_output = クロスバイナリの出力
4. メインバイナリに対して judge() を呼び出し(期待値をクロスバイナリの出力とする) **progress_barあり**
5. `JudgeOutcome { verdicts: outcome.verdicts.into_iter().filter(非AC).collect() }` でフィルタリングし `print_pretty` — 通番は 1/N 形式にリセットされる（フィールドがpublicなため手動構築可）

### Cargo.toml 自動登録

- `[[bin]]` エントリと `[package.metadata.cargo-compete.bin]` エントリを同時に追加
- bin name: `{contest}-{ファイル名stemのkebab変換}` 例: `abc440-a-brute`
- alias: ファイル名stemのkebab変換 例: `a-brute`
- `--cross` にファイル名だけを指定した場合は対象packageの `src/bin/` 配下を参照する（例: `--cross "a copy.rs"` は `src/bin/a copy.rs`）
- 既に同名binが登録済みでも、指定されたソースと `[[bin]].path` が異なる場合は正しいpathへ更新する

### 比較方法

- `a.yml`（テストスイート）の `match:` フィールドを使用（`Exact` / `Lines` / `Float` など）

### 出力フォーマット（クロスチェック）

クロスバイナリのサンプルテスト後、ランダムケースの判定結果を以下の形式で表示する。

#### AC
```
(サンプルチェックの最終行)

══════════════════════════════════════════
      cross-check binary sample tests
══════════════════════════════════════════
1/3 ("sample1") Accepted (0 ms)           ←progress bar
2/3 ("sample2") Accepted (0 ms)
3/3 ("sample3") Accepted (0 ms)

1/3 ("sample1") Accepted (0 ms)           ←print_pretty
stdin:
3 5
expected:
3
actual:
3

2/3 ("sample2") Accepted (0 ms)
stdin:
1 7
expected:
7
actual:
7

3/3 ("sample3") Accepted (0 ms)
stdin:
14 79
expected:
66
actual:
66

══════════════════════════════════════════
            cross-check tests
══════════════════════════════════════════
1/3 ("corner1") Accepted (5 ms)                         ←progress bar
2/3 ("corner2") Wrong Answer (8 ms)
3/3 ("corner3") Runtime Error (exit status: 1) (2 ms)

1/2 ("corner2") Wrong Answer (8 ms)
stdin:
{input}
expected:
{brute-force output}
actual:
{main binary output}

2/2 ("corner3") Runtime Error (exit status: 1) (2 ms)
stdin:
{input}
actual:
EMPTY

expected: a-copy ←AC以外がある場合のみ
actual: a        ←AC以外がある場合のみ

note: output beyond --display-limit (default: 4KiB; e.g. 152834 B) is truncated; change the limit with --display-limit
warning: skipped N unsupported constraint(s): {制約内容}  ← スキップがある場合のみ
error: {失敗件数}/{総件数} tests failed  ← 失敗がある場合のみ
```

**注記:**
- `{name}` は コーナーケースなら `corner1`, `corner2`, ...、ランダムケースなら `random1`, `random2`, ... の形式
- 各テストケースについて `print_pretty` の出力をそのまま使う（上記フォーマットはそこを指定するものではない）
- スキップした制約の警告は**末尾のみ**出力する（`judge()` + `out:None` ベースに変更後）

#### 末尾
```
warning: skipped N unsupported constraint(s): {制約内容}  ← スキップがある場合のみ
error: {失敗件数}/{総件数} tests failed  ← 失敗がある場合のみ
```

---

## 制約パースの詳細動作

- 制約文は LaTeX 形式で記述されており、`normalize_constraint()` で ASCII 化してからパース
- 変数名は小文字に統一して管理
- 認識・抽出するパターン:
  - 不等式（`1 \leq N \leq 10^5` 等の連鎖含む）
  - 列挙（`X は 0, 1 のいずれか` / `X \in \{a, b\}`）
  - 全相異なり（`A_1, ..., A_N は相異なる`）
    - distinct スコープは **len 方向／行内のみ**（多次元は各行独立に相異。
      行をまたいだ全要素相異は非保証）
    - 値域が len より狭く相異不能（`span < len`、len はテストケースごとに
      変動）なら **そのテストケースをその時点で棄却**（resample、corner は
      Random でバックフィルして件数維持）。problem 全体は中止しない
  - sum 上限（`X の総和は Y 以下`）
  - 文字列宣言（`S は 英小文字からなる長さ N の文字列`）
- スキップする制約（`random_test.skipped` に記録され末尾警告として表示）:
  - `\dfrac` / `\frac` / `\sqrt` を含む式
  - 不等号が見つからない自然文（例: `... は整数である` / `入力は全て整数`）
  - 上記いずれのパターンにも一致しないもの

### 文字列変数のデフォルト charset

デフォルト charset は **問題取得時（retrieve/new）の制約パースで yml の `values` に確定して書き込む**。
制約文から charset を特定できない一般の「文字列」宣言には
**英大文字 + 英小文字の 52 文字（A–Z, a–z）** を書き込む。

生成時は yml の `values` のみを参照する（yml-only 原則）。`type: Chars` なのに
`values` が無い yml（手編集で削除した場合など）はデフォルトで補完せず、警告して中断する。

| 制約文の条件（取得時） | yml に書き込む charset |
|------|---------|
| charset が特定できる（英小文字・英大文字・数字列・列挙など） | 特定した文字集合 |
| 特定できない一般の文字列宣言 | A–Z + a–z（52 文字） |

---

## yml スキーマ規約

`testcases/{problem}.yml` の `random_test:` セクションは、`format` 配列に `FormatBlock` を順に並べた構造で入力フォーマットを表現する。本セクションでは各ブロック種別とフィールド意味、命名規則を厳格に定義する。

### フィールドの意味（厳格）

| フィールド | 意味 |
|-----------|------|
| `len` | 配列の戦略適用対象となる長さ変数。Array 系戦略（ArrayMonoInc 等）が「行ごとに独立適用」する単位を一意化するために必要 |
| `height` | 3D 配列の中間次元 |
| `count` | 配列の外側次元・繰り返し回数。TestCases / Queries では本体ブロックの繰り返し回数 |
| `jagged` | Array が Jagged 配列であることを示す bool（後述） |

### `len` の軸と Chars 長の解決原則

- `Array` の最内次元は 1 つだけ保持する。整数配列は format 側 `array.len`、Chars 配列・グリッドは `vars[s].len` に保持し、format 側 `array.len` は重複定義しない。
- `Rows.len` は反復行数であり、行内に Chars 列がある場合の `vars[s].len`（文字列長）とは別の軸である。両方が存在してよい。
- `Scalars` / `TestCases` / `Queries` 自身は format dimension の `len` を持たない。ただし出力する Chars 変数は `vars[s].len` を持つ。
- Chars の `len` が literal または通常変数（例: `3`, `w`）なら厳密な共有長として扱い、同じ Chars 変数から出力する各文字列へ同じ長さを適用する。
- Chars の `len` が pipe 付き synthetic 変数（例: `|s|`）なら長さ範囲を表し、scalar、Array、Rows、TestCases / Queries 内を問わず、出力する Chars 1 要素ごとに長さを生成する。

| 形 | yml | 戦略の単位 |
|---|-----|-----------|
| 整数 1D | `array { base, len: n }` | 配列全体（len=n） |
| 整数 2D | `array { base, len: w, count: h }` | 各行（len=w） |
| 整数 3D | `array { base, len: w, height: h, count: f }` | 各最内行（len=w） |
| 固定幅 Chars 配列 / 2D グリッド | `array { base: s, count: h }` + vars `s: Chars, len: w` | 各文字列（共有長 w） |
| 可変長 Chars 配列 | `array { base: s, count: n }` + vars `s: Chars, len: "|s|"` | 各文字列（要素ごとに長さ生成） |
| Chars 3D | `array { base: s, height: h, count: f }` + vars `s: Chars, len: w` | 各文字列（共有長 w） |
| Rows（N-repeat タプル） | `rows { vars, len: m }` | 行（len=m）。Chars 列の文字列長は vars 側で別途解決 |
| Jagged | `array { base, len: l, count: n, jagged: true }` + vars `l: { range, sum_limit? }` | 各行（行長 = 動的 l_i） |

### yml 出力順序（内→外）

配列のフィールドは **内側次元 → 外側次元** の順で記述する。これは proconio の
`[[T; w]; h]` 構文の読み順とも一致する。

```yaml
# 整数 3D の例
- array:
    base: a
    len: w        # 最内（W）
    height: h     # 中間
    count: f      # 外側（F）
```

`ArrayBlock` の Rust 構造体のフィールド順も `base → len → height → count → jagged` に合わせる。

### dimension 長の読み取り

- 添字付き配列の dimension 長は、固定列挙か省略表記かを区別せず、同一 base の先頭・末尾 index から `末尾 - 先頭 + 1` として導出する。
- 例: `A_1 A_2 A_3` は `len: "3"`、`A_0 \cdots A_{N-1}` は `len: n`、`A_{-1} \cdots A_W` は `len: w+2` とする。
- `\ldots` / `\vdots` は dimension 長の根拠ではなく、明示されていない要素・行を越えて同一ブロックの終端 index を読むための区切りとして扱う。
- 整数 1D の横/縦表記、Chars 配列の外側、Rows の反復数、整数 2D、Chars グリッド、整数/Chars 3D は共通の端点式で dimension 長を取得し、矩形ブロックは共通の indexed span 走査後に型ごとの yml 表現へ変換する。
- Jagged の外側行数は同じ端点 span で取得するが、内側長は各行で異なる入力変数 `L_i` であるため `len: l` のままとする。Chars 単独の文字列長は入力形式に現れないため、制約パース由来の `vars[s].len` を維持する。
- yml の size field で評価可能な形に限定し、異なる二変数の差を必要とする index span は自動変換しない。

## parse パターン表

各行は parse 検出関数と yml 出力形式の組み合わせを示す。

| # | パターン | 検出関数 | HTML 例 | yml 出力（内→外順） | input.rs 出力 | 代表問題（HTML 例ごとに 2+） |
|---|---------|---------|--------|---------|--------------|---------|
| 1 | Scalars（単独 / 複数 / 添字付） | inline | (a) `N M K`<br>(b) `S_x S_y`<br>(c) `T` | `scalars { vars: [n, m, k] }` | `n: usize, m: usize, k: usize,` | (a) abc450/a, abc457/b<br>(b)  |
| 2 | Chars 単独（1 文字列） | inline (Scalars 扱い) | `S` | `scalars { vars: [s] }` + vars `s: Chars, len: n` | `s: Chars,` | abc441/b |
| 3 | 整数 1 次元配列 | `parse_1d_array_line` / `parse_vertical_scalars` | (a) `A_1 \cdots A_N`<br>(b) `A_1\vdots A_N`<br>(c) `A_1 A_2 A_3`（固定） | `array { base: a, len: n }` | `a: [usize; n],` | (a) abc450/a, abc457/b<br>(b)  |
| 4 | Chars 配列 / 2D グリッド | indexed span parse 後に Chars 型で変換 | (a) `S_1\cdots S_N`（各要素が可変長）<br>(b) `S_1\vdots S_H`（各要素が長さ W）<br>(c) `S_{i,1}\cdots S_{i,W}` over H | (a) `array { base: s, count: n }` + vars `s: Chars, len: "|s|"`<br>(b)(c) `array { base: s, count: h }` + vars `s: Chars, len: w` | `s: [Chars; count],` | (a) abc459/b<br>(c) abc453/d |
| 5 | 整数 2D 行列 | `parse_matrix_block`（共通 indexed span） | (a) `A_{i,1}\cdots A_{i,W}` over H<br>(b) `A_{1,1} A_{1,2} A_{1,3}\nA_{2,1}\dots`（全展開） | `array { base: a, len: w, count: h }` | `a: [[usize; w]; h],` | (a) abc450/c ＋ <br>(b) abc456/b ＋  |
| 6 | 整数 3D 配列 | `parse_3d_array_block`（整数） | `A_{f,h,1}\cdots A_{f,h,W}` | `array { base: a, len: w, height: h, count: f }` | `a: [[[usize; w]; h]; f],` |  |
| 7 | Chars 3D 配列 | `parse_3d_array_block`（Chars） | `S_{f,h,1}\cdots S_{f,h,W}` | `array { base: s, height: h, count: f }` + vars `s: Chars, len: w` | `s: [[Chars; h]; f],` | abc440/g  |
| 8 | Jagged 配列（入力に行サイズあり） | `parse_varlen_rows`（別行型は `fold_separate_len_rows` で同行型へ畳んでから検出） | (a) 同行 `L_i a_{i,1}\cdots a_{i,L_i}`<br>(b) 別行 `L_i\nX_{i,1}\cdots X_{i,L_i}` | `array { base: a, len: l, count: n, jagged: true }` + vars `l: { range, sum_limit? }` | `a: [[usize]; n],` | (a) abc457/c（sum あり）<br>(b) abc446/b（sum なし） |
| 9 | N-repeat タプル | `parse_n_repeat` | (a) `x_1 y_1\vdots x_M y_M`<br>(b) `L_i R_i C_i` over Q | `rows { vars: [x, y], len: m }`。Chars 列があれば `vars[c].len` も保持 | `xy: [(usize, usize); m],` 等 | abc442/c, abc440/d, abc450/e |
| 10 | TestCases | `parse_task_sections` 連動 | `\text{case}_T` 行 | `test_cases { count: t, format }` | `for _ in 0..t { input!{..} }` | abc440/c, abc443/d |
| 11 | Queries（単一ブロック・discriminator なし） | `parse_task_sections` 連動（`input_blocks.len() <= 2` 分岐） | `\mathrm{query}_i` 等のプレースホルダーがあるが、追加ブロックが 1 個しかなく識別子なし | `test_cases { count: q, format }`（Queries ではなく TestCases として出力） | `for _ in 0..q { input!{..} }` |  |
| 12 | Queries（数値 discriminator） | `parse_task_sections` 連動 | `1 a / 2 b` 等の繰り返し | `queries { count: q, types: [...] }` | `for _ in 0..q { match qt {..} }` | abc442/d, abc449/b |

**注記**:
- 行 1（Scalars）: 単独・複数・添字付は実装上 inline で同一の Scalars 出力に統合される
- 行 3（整数 1D）: `parse_1d_array_line` / `parse_vertical_scalars` は yml 出力形が同一のため統合表示
- 行 4（Chars 配列 / グリッド）: 外側の indexed span は共通で `array.count` に変換する。文字列長は制約由来の `vars[s].len` を用い、`|s|` なら要素ごとに生成し、`w` / literal なら共有する。
- 行 5（整数 2D）: 固定列挙・省略表記・固定列数かつ可変行数の混在表記は、いずれも共通 indexed span から `len` / `count` を導出する（abc456-b / abc448-g）
- 行 6（3D 整数）と行 7（3D Chars）は yml が異なる（W が format 側 or vars 側）ため別行とする
- 行 11（単一ブロック Queries）: `\mathrm{query}_i` 等のプレースホルダーが検出されても、per-query フォーマットブロックが 1 つだけで discriminator がない場合は意味的に TestCases と同じため、`TestCasesBlock` として出力する（`task.rs` の `input_blocks.len() <= 2` 分岐）。`count` 変数はプレースホルダーの subscript から抽出、なければ `q`
- 行 12（Queries）: 数値 discriminator のみが実用上機能する。

---

## Jagged 配列の仕様

### 定義

**Jagged 配列** = 「**入力に各行のサイズ `L_i` が含まれている可変長行配列**」。
master の `parse_varlen_rows` が検出するパターン全般がこれに該当する。

入力に長さが含まれない `S_1 \vdots S_H` のような均一行は Jagged ではない
（長さの扱いがランダム生成時に変わるため yml レベルで区別する）。

### 仕様

- **要素型**: usize / i64 のみ。Chars は対応しない（master と同じ方針 — 調査確認済）
- **入力フォーマット**: HTML 上は同行（`L_i a_{i,1} \cdots a_{i,L_i}`、abc457-c 等）と
  別行（`L_i\nX_{i,1}\cdots X_{i,L_i}`、abc446-b 等）の 2 形式があるが、**yml では区別しない**。
  ランダムテストの出力時も改行の入れ方は元の問題と一致しなくて良い
- **input.rs 生成**: `[[T]; n]` の proconio jagged シンタックス（手動 Vec<Vec<T>> ループは廃止）
- **yml 表現**: `array { base, len: l, count: n, jagged: true }` の `jagged: true` フラグで識別。
  `len: l` の `l` は「各行の長さを表す変数」と解釈
- **長さ変数 `l` の vars 側書式**:
  - 通常変数と**統一的に処理**: `l: { type: usize, range: [1, m], sum_limit: <値があれば> }`
  - 制約 `1 \le L_i \le M` → `l.range = [1, m]`
  - 制約 `\sum L_i \le X` → `l.sum_limit = X`（無条件で sum_limit に投入、Σ 特殊扱いはしない）
  - 冗長性回避: yml 上で「`l` 配列がランダム生成出力に含まれること」は明記しない
    （`jagged: true` フラグから推論する）
  - **sum_limit は必須ではない**（abc446-b は sum 制約なし）
- **生成時の挙動**:
  - `count = n` 行を生成。各行先頭に `l_i` を出力（生成側で勝手に出す）、続いて `l_i` 個の要素
  - `vars[l].sum_limit = L` があれば動的上限 `L / n` で `l_i` をクランプ（既存 sum_limit ルールと同じ）

---

## 非対応事項

| 問題 | 理由 |
|------|------|
| abc449/f | `H W h w N` の大文字/小文字重複。許容範囲内 |
| abc441/g | `t` の値と入力形式の対応が制約に書かれていないため HTML から生成不可 |
| abc451/e | 下三角行列（行ごとに幅が違うジャギー配列）→ 現スキーマ非対応 |
| abc450/b | 下三角行列（行ごとに幅が違うジャギー配列）→ 現スキーマ非対応（abc451/e と同型） |
| abc441/b | `w _ i は…文字列` のように添字に空白が入る文字列宣言は検出対象外 |
| abc443/e | `Σ N^2 <= 9e6` のような非線形総和制約は生成へ反映しない。セーフティ上限による中断も許容する |
| abc454/g | seed ベースの生成系問題。HTML から入力形式を決定できない |
| abc457/b | `sum_limit` を持つ `L` に対する `Y <= L_X` から、`Y` の上限または `sum_limit` を推論しない |
| abc457/c | `K <= Σ C_i L_i` のような重み付き総和上限を満たす生成は行わない。集約式を偽 `ordering` に近似しない |
| abc457/f | `D_i <= N-i` のような index 依存要素上限は生成へ反映しない。`d <= n` への弱化抽出も行わない |
| 関数式一般 | `min(...)` / `max(...)` の関数値を bound として評価しない。`min` / `max` 自体は変数名にせず、内部の入力変数は通常の変数抽出・`ordering` 抽出対象とする |
| abc442/f, abc445/b, abc445/g, abc455/b, abc456/e | 非対応フォーマット |
