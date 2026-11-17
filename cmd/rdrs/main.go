// Command rdrs is a self-hosted RSS reader.
package main

import (
	"context"
	"fmt"
	"io"
	"os"

	"github.com/henry40408/rdrs/internal/version"
)

func main() {
	os.Exit(run(context.Background(), os.Args[1:], os.LookupEnv, os.Stdout, os.Stderr))
}

// run is the testable entry point; it returns the process exit code.
func run(_ context.Context, _ []string, _ func(string) (string, bool), stdout, _ io.Writer) int {
	fmt.Fprintln(stdout, "rdrs", version.Version())
	return 0
}
