package main

import (
	l36io "github.com/incyashraj/krate/packages/sdk-go/krate/io"
	l36locale "github.com/incyashraj/krate/packages/sdk-go/krate/locale"
	l36time "github.com/incyashraj/krate/packages/sdk-go/krate/time"
)

func main() {
	loc := l36locale.Current()
	tz := l36locale.Timezone()
	now := l36time.NowMillis()
	date := l36locale.FormatDate(now, tz, l36locale.DateStyleMedium, loc)

	_ = l36io.Println("app=krate-go-clock")
	_ = l36io.Println("locale=" + loc.BCP47)
	_ = l36io.Println("timezone=" + tz)
	_ = l36io.Println("date=" + date)
}
