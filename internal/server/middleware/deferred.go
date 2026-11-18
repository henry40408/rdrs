package middleware

import (
	"bytes"
	"net/http"
)

// deferredWriter either passes a response through or, when shouldBuffer says
// so once the head is known, holds the body until finish so a layer can
// rewrite it. Layers that edit bodies use it so streaming responses are never
// buffered.
type deferredWriter struct {
	http.ResponseWriter
	shouldBuffer func(status int, header http.Header) bool
	maxBody      int // 0 means no limit
	// spill makes a body over maxBody stream through unchanged instead of
	// being dropped (overflow).
	spill bool

	decided   bool
	buffering bool
	status    int
	body      bytes.Buffer
	overflow  bool
}

func (w *deferredWriter) WriteHeader(status int) {
	// Informational heads (1xx) are not the final head.
	if status < 200 {
		w.ResponseWriter.WriteHeader(status)
		return
	}
	if w.decided {
		return
	}
	w.decided = true
	w.status = status
	if w.shouldBuffer(status, w.Header()) {
		w.buffering = true
		return
	}
	w.ResponseWriter.WriteHeader(status)
}

func (w *deferredWriter) Write(data []byte) (int, error) {
	if !w.decided {
		w.WriteHeader(http.StatusOK)
	}
	if !w.buffering {
		return w.ResponseWriter.Write(data)
	}
	if w.spill && w.maxBody > 0 && w.body.Len()+len(data) > w.maxBody {
		w.buffering = false
		w.ResponseWriter.WriteHeader(w.status)
		_, _ = w.ResponseWriter.Write(w.body.Bytes())
		w.body.Reset()
		return w.ResponseWriter.Write(data)
	}
	if w.maxBody > 0 && w.body.Len()+len(data) > w.maxBody {
		w.overflow = true
		w.body.Reset()
	}
	if !w.overflow {
		w.body.Write(data)
	}
	return len(data), nil
}

func (w *deferredWriter) Flush() {
	if !w.buffering {
		_ = http.NewResponseController(w.ResponseWriter).Flush()
	}
}

func (w *deferredWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }

// held reports whether a response is waiting to be rewritten. Call it after
// the handler returned; a handler that wrote nothing counts as an empty 200.
func (w *deferredWriter) held() bool {
	if !w.decided {
		w.WriteHeader(http.StatusOK)
	}
	return w.buffering
}
