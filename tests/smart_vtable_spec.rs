// tests/smart_vtable_spec.rs
//
// Copyright (c) 2026 Synome
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Smart VTable & COM Runtime Specification Tests
//
// このファイルは、kcom v0.5 で導入された「Smart VTable (Factory Pattern)」が
// 意図通りに動作し、ゼロコスト・ゼロコピー・型安全性を満たしていることを検証します。

use core::ffi::c_void;
use core::mem;
use kcom::*;
use kcom::vtable::ComInterfaceInfo; // IID取得のために必要

// =========================================================================
// 1. Test Fixtures (Definitions)
// =========================================================================

// --- Interface Definitions ---

declare_com_interface! {
    /// プライマリとして使用するインターフェース
    pub trait ISmartFoo: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1111_1111,
            data2: 0x0000,
            data3: 0x0000,
            data4: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
        };
        fn foo(&self) -> NTSTATUS;
    }
}

declare_com_interface! {
    /// セカンダリとして使用するインターフェース (Thunkの検証用)
    pub trait ISmartBar: IUnknown {
        const IID: GUID = GUID {
            data1: 0x2222_2222,
            data2: 0x0000,
            data3: 0x0000,
            data4: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02],
        };
        // 戻り値で計算結果を返すことで、thisポインタが正しいか確認する
        fn bar(&self, val: u32) -> u32;
    }
}

// --- Driver Implementation ---

struct MyDriver {
    magic: u32,
}

impl ISmartFoo for MyDriver {
    fn foo(&self) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

impl ISmartBar for MyDriver {
    fn bar(&self, val: u32) -> u32 {
        // ここで self.magic にアクセスできる = thisポインタが正しく調整されている
        self.magic.wrapping_add(val)
    }
}

// --- COM Implementation (Primary) ---

// Smart VTable パターンにより、ユーザー実装側では
// `new::<Self>()` を呼ぶだけで VTable がコンパイル時に確定する。
impl_com_interface! {
    impl MyDriver: ISmartFoo {
        parent = IUnknownVtbl,
        secondaries = (ISmartBar),
        // 【修正】必須の `methods` を指定
        methods = [foo],
    }
}

// --- COM Implementation (Secondary) ---

impl_com_interface_multiple! {
    impl MyDriver: ISmartBar {
        parent = IUnknownVtbl,
        primary = ISmartFoo,
        index = 0,
        secondaries = (ISmartBar),
        methods = [bar],
    }
}

// =========================================================================
// 2. Verification Tests
// =========================================================================

/// 🏗️ TEST 1: Const Construction Check
/// VTable の構築が完全にコンパイル時定数として処理されることを証明する。
/// これがコンパイルエラーになる場合、"Smart VTable" は達成されていない。
#[test]
fn vtable_is_const_constructible() {
    // static 変数として定義可能か？
    static STATIC_VTABLE_FOO: ISmartFooVtbl = ISmartFooVtbl::new::<MyDriver>();
    
    // アドレスが静的領域にあることを確認 (nullでない)
    let ptr = &STATIC_VTABLE_FOO as *const ISmartFooVtbl;
    assert!(!ptr.is_null());

    // 実際に ComImpl で使用されている VTABLE に到達できるか確認
    let _impl_vtbl = <MyDriver as ComImpl<ISmartFooVtbl>>::VTABLE;
}

/// 🧬 TEST 2: Thunk / Offset Correctness Check
/// 多重継承したインターフェース経由でメソッドを呼んだ際、
/// `this` ポインタが正しく `MyDriver` の先頭に戻されているか検証する。
#[test]
fn secondary_interface_adjusts_this_pointer_correctly() {
    let driver = MyDriver { magic: 0xDEAD_BEEF };
    
    // ComObjectN (多重継承コンテナ) を生成
    // 返ってくるポインタは Primary (ISmartFoo) のもの
    let raw_ptr = ComObjectN::<MyDriver, ISmartFooVtbl, (ISmartBarVtbl,)>::new(driver).unwrap();
    let foo_ptr = raw_ptr as *mut ISmartFooRaw;

    unsafe {
        // 1. Primary Interface Call
        let foo_vtbl = (*foo_ptr).lpVtbl;
        let status = ((*foo_vtbl).foo)(foo_ptr as *mut c_void);
        assert_eq!(status, STATUS_SUCCESS);

        // 2. QueryInterface for Secondary (ISmartBar)
        let mut bar_ptr_void: *mut c_void = core::ptr::null_mut();
        // 【修正】IIDの取得方法を Raw構造体経由に変更し、曖昧さを排除
        let qi_status = ((*foo_vtbl).parent.QueryInterface)(
            foo_ptr as *mut c_void,
            &<ISmartBarRaw as ComInterfaceInfo>::IID,
            &mut bar_ptr_void
        );
        assert_eq!(qi_status, STATUS_SUCCESS);
        assert!(!bar_ptr_void.is_null());
        
        // ポインタがプライマリと異なることを確認 (オフセットされているはず)
        assert_ne!(raw_ptr, bar_ptr_void, "Secondary pointer must be offset from primary");

        // 3. Secondary Interface Call
        let bar_ptr = bar_ptr_void as *mut ISmartBarRaw;
        let bar_vtbl = (*bar_ptr).lpVtbl;
        
        // ★ 最重要検証ポイント
        // shim 内部で `container_of` (from_secondary_ptr) の計算が狂っていると、
        // `self.magic` (0xDEAD_BEEF) が正しく読めず、ゴミデータになるかクラッシュする。
        let result = ((*bar_vtbl).bar)(bar_ptr_void, 1);

        // 正しく this が調整されていれば、magic + 1 が返る
        // 【修正】型サフィックス _u32 を追加して曖昧さを排除
        assert_eq!(result, 0xDEAD_BEEF_u32.wrapping_add(1));

        // クリーンアップ (QI で増えた参照 + 作成時の参照)
        let _release_cnt_1 = ((*foo_vtbl).parent.Release)(foo_ptr as *mut c_void); // QI ref release
        // 残り1
        let release_cnt_2 = ((*foo_vtbl).parent.Release)(foo_ptr as *mut c_void); // Owner ref release
        assert_eq!(release_cnt_2, 0); // 0 になって解放されるはず
    }
}

/// 📏 TEST 3: ABI Layout Consistency Check
/// 生成された VTable 構造体が、C言語のメモリレイアウトと一致しているか検証する。
#[test]
fn vtable_layout_matches_c_abi() {
    // C言語での期待レイアウト (vptr配列)
    #[repr(C)]
    struct ExpectedFooVtbl {
        // parent: IUnknownVtbl (3 ptrs)
        qi: usize,
        addref: usize,
        release: usize,
        // foo: fn (1 ptr)
        foo: usize,
    }

    assert_eq!(
        mem::size_of::<ISmartFooVtbl>(),
        mem::size_of::<ExpectedFooVtbl>(),
        "VTable size mismatch with C ABI"
    );
    assert_eq!(
        mem::align_of::<ISmartFooVtbl>(),
        mem::align_of::<ExpectedFooVtbl>(),
        "VTable alignment mismatch with C ABI"
    );
    
    // フィールドオフセット確認 (foo は 4番目のポインタ)
    // IUnknown (3 ptrs) * 8 bytes = 24 bytes offset (on 64bit)
    let foo_offset = core::mem::offset_of!(ISmartFooVtbl, foo);
    
    let expected_offset = mem::size_of::<usize>() * 3;
    assert_eq!(foo_offset, expected_offset, "Method offset mismatch");
}
