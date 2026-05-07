# Hybrid Shared Memory Ring (HSMR) アーキテクチャ仕様書

**Version:** 1.0  
**Target OS:** Windows 10/11 (x64/ARM64)  
**Kernel Mode:** KMDF / WDM  

## 1. 概要
本アーキテクチャは、Windowsにおける標準的な `IOCTL` や `IoRing` のオーバーヘッドを排除し、特定のデバイス制御およびプロセス間通信（IPC）において極限の低レイテンシを実現するための設計である。

ユーザー空間とカーネル空間で物理メモリを共有し、ロックフリーリングバッファと適応的ポーリング（Adaptive Polling）を組み合わせることで、**システムコールの発行回数を理論上の最小値（ゼロに近接）** に抑える。

## 2. MVP パラメータ (固定値)

MVPではパラメータを固定し、実装を簡素化する。

*   **リングエントリ数:** `4096`（2の累乗）
*   **パケットサイズ:** `64B`（固定長）
*   **リング総容量:** `256KB`（4096 * 64B）
*   **Overflow エントリ数:** `1024`
*   **Doorbell:** **Event方式**（ユーザーがイベントを作成し、必要時のみ `SetEvent`）

### 2.1 MVP2: Large Ring (2MB)

MVP2では **2MB Large Page リング**を採用し、満杯条件を緩和する。

*   **リングエントリ数:** `32768`（2MB / 64B）
*   **パケットサイズ:** `64B`（固定長）
*   **リング総容量:** `2MB`
*   **Doorbell:** **Event方式**（MVPと同じ）
*   **Overflow:** 通常オーバーフロー用途ではなく **OOB（緊急制御）** に寄せる

## 3. アーキテクチャ構成図

```mermaid
graph TD
    subgraph User Mode [User Space]
        Producer[Producer Thread]
        UserView[Mapped View (Virtual Address)]
    end

    subgraph Kernel Mode [Kernel Space]
        Driver[Device Driver]
        KThread[Dedicated Polling Thread]
        MDL[MDL / NonPagedPool]
    end

    Producer -->|Write Command| UserView
    UserView <==>|Shared Physical Memory| MDL
    KThread -->|Read Command| MDL
    KThread -->|Process| Driver

    note1[Signal Path]
    Producer -.->|Doorbell (IOCTL/Event)| KThread
    KThread -.->|Adaptive Wait| note1
```

## 4. 主要技術仕様

### 4.1 メモリ管理戦略 (Kernel-Hosted)
従来の「User確保→Kernelロック」ではなく、**「Kernel確保→Userマップ」**を採用する。

*   **確保:** カーネル側で `MmAllocatePagesForMdl` または `ExAllocatePool` を使用し、非ページプール（NonPagedPool）を確保する。
*   **マッピング:** `MmMapLockedPagesSpecifyCache (UserMode)` を使用してユーザー空間の仮想アドレスにマッピングする。
*   **利点:** プロセスクラッシュ時のクリーンアップが堅牢であり、物理メモリの連続性やラージページ（2MB/1GB）の活用が可能。

### 4.2 データ構造設計 (Cache-Aware)
CPUキャッシュラインの競合（False Sharing）を防ぐため、64バイトアライメントを強制する。

#### リングバッファ制御構造体 (C/C++)
```c
#define CACHE_LINE_SIZE 64
#define HSMR_RING_SIZE 4096
#define HSMR_RING_SIZE_LARGE 32768
#define HSMR_OVERFLOW_ENTRIES 1024
#define HSMR_PACKET_BYTES 64

typedef struct _HSMR_PACKET {
    UCHAR Bytes[HSMR_PACKET_BYTES];
} HSMR_PACKET, *PHSMR_PACKET;

typedef struct _HSMR_CONTROL {
    // --- Kernel Write / User Read Area ---
    __declspec(align(CACHE_LINE_SIZE)) volatile LONG HeadIndex; // 読み取り位置
    volatile LONG KernelStatus;     // 0: Running, 1: Sleeping

    // --- User Write / Kernel Read Area ---
    __declspec(align(CACHE_LINE_SIZE)) volatile LONG TailIndex; // 書き込み位置

    // --- Overflow Lane Control ---
    __declspec(align(CACHE_LINE_SIZE)) volatile LONG OverflowHead;
    volatile LONG OverflowTail;
    
    // --- Data Area ---
    // リングバッファの実体（オフセット管理またはポインタ）
    HSMR_PACKET Packets[HSMR_RING_SIZE]; 
    HSMR_PACKET OverflowPackets[HSMR_OVERFLOW_ENTRIES];

} HSMR_CONTROL, *PHSMR_CONTROL;
```

### 4.3 通信プロトコル (Adaptive Hybrid Signaling)

レイテンシとCPU負荷のトレードオフを動的に管理する「ハイブリッド」方式を採用する。

| 状態 | カーネル側の動作 | ユーザー側の動作 | レイテンシ | CPU負荷 |
| :--- | :--- | :--- | :--- | :--- |
| **高負荷** | **Busy Loop:** データ取得待機中、スピン（`_mm_pause`）する。 | **No Syscall:** バッファに書き込み、Tail更新のみ。 | **極小 (<1us)** | 高 (1コア占有) |
| **低負荷** | **Sleep:** 一定時間データがなければ `KeWaitForSingleObject` で待機。 | **Doorbell:** `KernelStatus` を見て、Sleep中ならイベント発火。 | 中 (数us〜) | 低 |

---

### 4.4 満杯時プロトコル (Shadow Head + Overflow Lane)

MVP以降の「準ロスレスで高速」な満杯対策として、**Shadow Head（Cached Head）+ Overflow Lane** を採用する。

**Shadow Head による論理Credit (Primary Ring)**
* 共有の `Credits` 変数は持たない。
* ユーザーは `CachedHead` をローカルに保持し、空き数を  
  `Space = RingSize - (Tail - CachedHead)` で算出する。
* `Space > 0` なら Primary に書き込み可能。
* `Space <= 0` のときのみ `HeadIndex` を共有メモリから再読して `CachedHead` を更新する。
* それでも空きがなければ Overflow にフォールバックする。

**Overflow Lane (Secondary Ring)**
* Primary が満杯のときのみ使用する固定サイズのセカンダリリング。
* カーネルは通常 Primary を処理し、Primary が空のときに Overflow を吸い上げる。
* グローバル順序が必要な場合は、**パケット内に `seq` を持たせて安定化**する。

**待機 (Space-Available Event / Optional)**
* Primary も Overflow も満杯ならユーザーは `SpaceAvailableEvent` を待機。
* カーネルが `Head` を進めたタイミングで `SetEvent` し、待機側を起床。

**補足 (Large Ring と OOB)**
* 2MB級の Large Page リングで Primary 容量を増やせる場合、Overflow は「通常オーバーフロー」ではなく OOB（緊急制御）用途に切り替える運用も可能。
* この場合、Overflow は優先度逆転を避けるため **別処理パス** とする。

> これにより「通常はゼロSyscall」「バースト時のみオーバーフロー」を実現し、  
> ロスレスに近い形で高スループットを維持できる。

### 4.5 Shadow Head (Cached Head) 最適化

`HeadIndex` の共有メモリ読取りを毎回行わず、**ユーザー側のローカルキャッシュ**を使って空き容量を推定する。

* Shared: カーネルは処理が進んだら `HeadIndex` を更新。
* Local: ユーザーは前回確認した `HeadIndex` を `CachedHead` として保持。
* Check: `Space = RingSize - (Tail - CachedHead)` で判定。
* 空きがない場合のみ `HeadIndex` を再読して `CachedHead` を更新。

この方式は「共有読み取りをほぼゼロ化」できるため、MVP以降の標準最適化として採用する。

## 5. 実装詳細フロー

### Phase 1: 初期化 (Setup)

1.  **User:** `CreateFile` でドライバハンドルを取得。
2.  **User:** `DeviceIoControl` (INIT_RING) を発行。
3.  **Kernel:**
    *   `IoAllocateMdl` / `MmBuildMdlForNonPagedPool` で共有メモリを準備。
    *   `MmMapLockedPagesSpecifyCache` でユーザー空間アドレスを生成。
    *   `PsCreateSystemThread` で専用のポーリングスレッドを起動。
    *   ユーザー空間アドレスを `OutputBuffer` として返却。

### Phase 2: データ送信 (User -> Kernel)

```cpp
// User Mode Producer Logic
void SubmitCommand(Command cmd) {
    // 1. バッファへの書き込み
    Ring->Packets[Ring->TailIndex & MASK] = cmd;
    
    // 2. メモリバリア (書き込み順序保証)
    std::atomic_thread_fence(std::memory_order_release);
    
    // 3. Tail更新 (Atomic)
    LONG oldTail = InterlockedIncrement(&Ring->TailIndex);
    
    // 4. ドアベル判定 (Kernelが寝ている時だけ起こす)
    if (Ring->KernelStatus == STATUS_SLEEPING) {
        // ここでのみシステムコール発生
        SetEvent(hDoorbellEvent); 
    }
}
```

### Phase 3: データ受信と処理 (Kernel Thread)

```cpp
// Kernel Mode Consumer Logic
void PollingThread(PVOID Context) {
    // CPUアフィニティ設定 (例: Core 2に固定)
    KAFFINITY affinity = (1ULL << 2);
    KeSetSystemAffinityThread(affinity);

    while (!bStop) {
        LONG localHead = Ring->HeadIndex;
        LONG localTail = Ring->TailIndex; // Volatile Read

        if (localHead != localTail) {
            // --- ACTIVE STATE ---
            Ring->KernelStatus = STATUS_RUNNING;
            
            // バッチ処理
            while (localHead != localTail) {
                // セキュリティ対策: ローカルコピー
                HSMR_PACKET packet = Ring->Packets[localHead & MASK];
                ProcessPacket(&packet); // 実際の処理
                
                localHead++;
            }
            
            // Head更新 (Userへの通知)
            InterlockedExchange(&Ring->HeadIndex, localHead);
            
            // ポーリングタイマーリセット
            pollParams.Reset();
        } 
        else {
            // --- IDLE STATE ---
            if (pollParams.ShouldSpin()) {
                _mm_pause(); // 電力消費を抑えつつ待機
            } 
            else {
                // スリープ遷移
                Ring->KernelStatus = STATUS_SLEEPING;
                KeMemoryBarrier(); // 状態書き込みを確定
                
                // Race Condition対策: 寝る直前に再チェック
                if (Ring->HeadIndex == Ring->TailIndex) {
                    KeWaitForSingleObject(KEvent, Executive, KernelMode, FALSE, NULL);
                }
                Ring->KernelStatus = STATUS_RUNNING;
            }
        }
    }
}
```

## 6. セキュリティ考慮事項 (TOCTOU対策)

共有メモリはユーザーモードプロセスから「いつでも」書き換え可能であるため、以下の対策を必須とする。

1.  **二重参照の禁止:**
    カーネルが `Ring->Packets[i].Value` を検証（if check）した後、使用する際にもう一度共有メモリから読み込んではならない。必ず**スタック上のローカル変数に `memcpy` してから検証・使用**すること。
2.  **境界チェック:**
    `Head` や `Tail` はオーバーフローする可能性があるため、常にリングサイズでのマスク処理（`Index & (SIZE - 1)`）を行い、配列外アクセスを防ぐ。
3.  **Sanitization:**
    ポインタが含まれる場合、それがカーネル空間を指していないか `ProbeForRead/Write` 相当のチェックを自前で行うか、そもそもポインタではなくハンドル/IDを使用する設計にする。

## 7. パフォーマンス・チューニング

1.  **CPU Affinity (Pinning):**
    ユーザー空間のスレッドとカーネルスレッドを、**L3キャッシュを共有する同一物理プロセッサ内の別コア**に固定することで、キャッシュヒット率を最大化する。
    *   **運用ルール:** カーネル側のポーリングスレッドだけでなく、**ユーザー側Producerスレッドも同じL3を共有するコア**に割り当てる。
        例: Core 2 が物理コア1のHTなら、Producerは Core 3 に固定する。
2.  **Huge Pages (Large Page):**
    カーネルドライバ側でメモリ確保時にラージページを使用できれば、TLBミスを削減できる（要: レジストリ設定やドライバフラグ）。
3.  **Batch Processing:**
    カーネルスレッドが起床した際、1つ処理してループに戻るのではなく、溜まっているコマンドをまとめて処理（Batch Dequeue）することで、メモリバリアのコストを償却する。
