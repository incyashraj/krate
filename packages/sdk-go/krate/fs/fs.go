package fs

import (
	"errors"

	layer36 "github.com/incyashraj/layer6x6/packages/sdk-go/layer36"
)

type OpenMode string

const (
	OpenModeRead      OpenMode = "read"
	OpenModeWrite     OpenMode = "write"
	OpenModeReadWrite OpenMode = "read-write"
	OpenModeAppend    OpenMode = "append"
)

type FileStat struct {
	Size           uint64
	ModifiedMillis uint64
	IsDir          bool
}

type File interface {
	Read(n uint32) ([]byte, error)
	Write(bytes []byte) (uint32, error)
	SeekSet(pos uint64) (uint64, error)
	SeekEnd() (uint64, error)
	Stat() (FileStat, error)
}

var (
	OpenHook       = func(string, OpenMode) (File, error) { return nil, layer36.ErrGeneratedBindingsMissing }
	StatHook       = func(string) (FileStat, error) { return FileStat{}, layer36.ErrGeneratedBindingsMissing }
	ListHook       = func(string) ([]string, error) { return nil, layer36.ErrGeneratedBindingsMissing }
	RemoveFileHook = func(string) error { return layer36.ErrGeneratedBindingsMissing }
	RemoveDirHook  = func(string) error { return layer36.ErrGeneratedBindingsMissing }
	MkdirHook      = func(string) error { return layer36.ErrGeneratedBindingsMissing }
	RenameHook     = func(string, string) error { return layer36.ErrGeneratedBindingsMissing }
)

func Open(path string, mode OpenMode) (File, error) {
	return OpenHook(path, mode)
}

func Stat(path string) (FileStat, error) {
	return StatHook(path)
}

func List(path string) ([]string, error) {
	return ListHook(path)
}

func RemoveFile(path string) error {
	return RemoveFileHook(path)
}

func RemoveDir(path string) error {
	return RemoveDirHook(path)
}

func Mkdir(path string) error {
	return MkdirHook(path)
}

func Rename(from string, to string) error {
	return RenameHook(from, to)
}

func Read(path string) ([]byte, error) {
	file, err := Open(path, OpenModeRead)
	if err != nil {
		return nil, err
	}
	if file == nil {
		return nil, errors.New("layer36 fs: open returned nil file")
	}

	return file.Read(4 * 1024 * 1024)
}

func ReadText(path string) (string, error) {
	bytes, err := Read(path)
	if err != nil {
		return "", err
	}

	return string(bytes), nil
}

func WriteText(path string, value string) (uint32, error) {
	file, err := Open(path, OpenModeWrite)
	if err != nil {
		return 0, err
	}
	if file == nil {
		return 0, errors.New("layer36 fs: open returned nil file")
	}

	return file.Write([]byte(value))
}
