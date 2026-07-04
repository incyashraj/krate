package time

func NowMillis() uint64 {
	return NowMillisHook()
}

func MonotonicNanos() uint64 {
	return MonotonicNanosHook()
}

func SleepMillis(millis uint32) {
	SleepMillisHook(millis)
}

var (
	NowMillisHook      = func() uint64 { return 0 }
	MonotonicNanosHook = func() uint64 { return 0 }
	SleepMillisHook    = func(uint32) {}
)
