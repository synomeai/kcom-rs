#include <iostream>
#include <chrono>
#include <atomic>
#include <memory>
#include <type_traits>

// Windows COM ABI (stdcall) をエミュレート
#if defined(_WIN32)
#define STDMETHODCALLTYPE __stdcall
#else
#define STDMETHODCALLTYPE
#endif

#if defined(_MSC_VER)
#define NOINLINE __declspec(noinline)
#elif defined(__GNUC__) || defined(__clang__)
#define NOINLINE __attribute__((noinline))
#else
#define NOINLINE
#endif

static volatile int g_sink = 0;
static constexpr int WARMUP_ITERATIONS = 100000;

// =========================================================
// 1. Manual COM Async Operation (The "Hardcore" C++ way)
// =========================================================

struct IAsyncOperation {
    virtual unsigned long STDMETHODCALLTYPE AddRef() = 0;
    virtual unsigned long STDMETHODCALLTYPE Release() = 0;
    virtual int STDMETHODCALLTYPE GetStatus(int* status) = 0;
    virtual int STDMETHODCALLTYPE GetResult(int* result) = 0;
};

class AsyncOperationCompleted : public IAsyncOperation {
    std::atomic<unsigned long> ref_count_;
    int result_;

public:
    explicit AsyncOperationCompleted(int result)
        : ref_count_(1), result_(result) {}

    unsigned long STDMETHODCALLTYPE AddRef() override {
        return ref_count_.fetch_add(1, std::memory_order_relaxed) + 1;
    }

    unsigned long STDMETHODCALLTYPE Release() override {
        unsigned long count = ref_count_.fetch_sub(1, std::memory_order_release) - 1;
        if (count == 0) {
            std::atomic_thread_fence(std::memory_order_acquire);
            delete this;
        }
        return count;
    }

    NOINLINE int STDMETHODCALLTYPE GetStatus(int* status) override {
        *status = 1; // Completed
        return 0; // S_OK
    }

    NOINLINE int STDMETHODCALLTYPE GetResult(int* result) override {
        *result = result_;
        return 0; // S_OK
    }
};

struct IMyAsyncOp {
    virtual unsigned long STDMETHODCALLTYPE AddRef() = 0;
    virtual unsigned long STDMETHODCALLTYPE Release() = 0;
    virtual IAsyncOperation* STDMETHODCALLTYPE GetStatusAsync() = 0;
};

class ManualComImpl : public IMyAsyncOp {
    std::atomic<unsigned long> ref_count_;

public:
    ManualComImpl() : ref_count_(1) {}

    unsigned long STDMETHODCALLTYPE AddRef() override {
        return ref_count_.fetch_add(1, std::memory_order_relaxed) + 1;
    }

    unsigned long STDMETHODCALLTYPE Release() override {
        unsigned long count = ref_count_.fetch_sub(1, std::memory_order_release) - 1;
        if (count == 0) {
            std::atomic_thread_fence(std::memory_order_acquire);
            delete this;
        }
        return count;
    }

    NOINLINE IAsyncOperation* STDMETHODCALLTYPE GetStatusAsync() override {
        return new AsyncOperationCompleted(1);
    }
};

// =========================================================
// 2. Modern C++ Implementation (Baseline)
// =========================================================

struct ReadyFuture {
    int value;
};

class ModernImpl {
public:
    NOINLINE int GetStatus() {
        return 1;
    }
};

// =========================================================
// Benchmarking Utilities
// - Warmup + atomic_signal_fence to reduce measurement skew from optimizations.
// =========================================================

template <class T>
void do_not_optimize(T&& datum) {
    using BaseType = typename std::remove_reference<T>::type;
    volatile BaseType* p = &datum;
    (void)p;
}

template <typename Func>
double measure_ns_raw(int iterations, Func func) {
    for (int i = 0; i < WARMUP_ITERATIONS; ++i) {
        func();
        std::atomic_signal_fence(std::memory_order_seq_cst);
    }
    auto start = std::chrono::high_resolution_clock::now();
    for (int i = 0; i < iterations; ++i) {
        func();
        std::atomic_signal_fence(std::memory_order_seq_cst);
    }
    auto end = std::chrono::high_resolution_clock::now();
    auto duration = std::chrono::duration_cast<std::chrono::nanoseconds>(end - start).count();
    return static_cast<double>(duration) / iterations;
}

template <typename Func>
double measure_ns(const char* name, int iterations, double baseline, Func func) {
    double avg = measure_ns_raw(iterations, func);
    double adj = avg > baseline ? (avg - baseline) : 0.0;
    std::cout << "[" << name << "] Average: " << avg << " ns"
              << " (adj " << adj << " ns)" << std::endl;
    return adj;
}

int main() {
    const int ITERATIONS = 10000000; // 10M loops

    std::cout << "Running C++ Async Benchmarks (" << ITERATIONS << " iterations)..." << std::endl;
    std::cout << "-----------------------------------------------------" << std::endl;
    double baseline = measure_ns_raw(ITERATIONS, []() { g_sink += 1; });
    std::cout << "[Cpp_Empty_Loop] Average: " << baseline << " ns" << std::endl;

    // Prepare a COM object.
    IMyAsyncOp* raw_obj = new ManualComImpl();

    // --- Allocation Benchmark ---

    // 1. Manual COM: async operation allocation
    measure_ns("Cpp_AsyncOp_New", ITERATIONS, baseline, [raw_obj]() {
        IAsyncOperation* op = raw_obj->GetStatusAsync();
        op->Release();
    });

    // 2. Modern C++: new (ready state)
    measure_ns("Cpp_New_Ready", ITERATIONS, baseline, []() {
        auto ptr = new ReadyFuture{1};
        do_not_optimize(*ptr);
        delete ptr;
    });

    // 3. Modern C++: make_shared (ready state)
    measure_ns("Cpp_Make_Shared_Ready", ITERATIONS, baseline, []() {
        auto ptr = std::make_shared<ReadyFuture>(ReadyFuture{1});
        do_not_optimize(ptr);
    });

    // --- Dispatch Benchmark ---

    IAsyncOperation* op = raw_obj->GetStatusAsync();

    // 3. Virtual Method Call (COM ABI)
    measure_ns("Cpp_AsyncOp_GetStatus", ITERATIONS, baseline, [op]() {
        int status = 0;
        op->GetStatus(&status);
        do_not_optimize(status);
    });

    op->Release();

    // 4. Native direct call
    ModernImpl native;
    measure_ns("Cpp_Native_Call", ITERATIONS, baseline, [&native]() {
        g_sink += native.GetStatus();
        do_not_optimize(g_sink);
    });

    raw_obj->Release();

    return 0;
}
