package main

import (
	krateio "github.com/incyashraj/krate/packages/sdk-go/krate/io"
	kratelocale "github.com/incyashraj/krate/packages/sdk-go/krate/locale"
	kratetime "github.com/incyashraj/krate/packages/sdk-go/krate/time"
)

func main() {
	loc := kratelocale.Current()
	tz := kratelocale.Timezone()
	now := kratetime.NowMillis()
	date := kratelocale.FormatDate(now, tz, kratelocale.DateStyleMedium, loc)

	_ = krateio.Println("app=krate-go-clock")
	_ = krateio.Println("locale=" + loc.BCP47)
	_ = krateio.Println("timezone=" + tz)
	_ = krateio.Println("date=" + date)
}
