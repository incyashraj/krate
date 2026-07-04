package locale

type LocaleID struct {
	BCP47 string
}

type DateStyle string

const (
	DateStyleShort  DateStyle = "short"
	DateStyleMedium DateStyle = "medium"
	DateStyleLong   DateStyle = "long"
	DateStyleFull   DateStyle = "full"
)

type NumberStyle string

const (
	NumberStyleDecimal  NumberStyle = "decimal"
	NumberStylePercent  NumberStyle = "percent"
	NumberStyleCurrency NumberStyle = "currency"
)

var (
	CurrentHook      = func() LocaleID { return LocaleID{BCP47: "und"} }
	TimezoneHook     = func() string { return "UTC" }
	FormatDateHook   = func(uint64, string, DateStyle, LocaleID) string { return "" }
	FormatNumberHook = func(float64, NumberStyle, LocaleID) string { return "" }
)

func Current() LocaleID {
	return CurrentHook()
}

func Timezone() string {
	return TimezoneHook()
}

func FormatDate(millis uint64, tz string, style DateStyle, loc LocaleID) string {
	return FormatDateHook(millis, tz, style, loc)
}

func FormatNumber(value float64, style NumberStyle, loc LocaleID) string {
	return FormatNumberHook(value, style, loc)
}
