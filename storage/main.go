// spatiand-storage: Spatiand's storage service.
//
// rclone, linked in as a library (docs/spatiand-apps.md): every backend rclone has, and the
// list of remotes, held by one process that every Spatiand application asks for files. It
// answers on 127.0.0.1 only, behind a token, in two ways:
//
//	/rc/<method>        rclone's own remote-control calls, JSON in and out: listing,
//	                    copying, moving, deleting, and setting remotes up (config/create and
//	                    its questions, one at a time, as `rclone config` asks them)
//	/dav/<remote>/...   each remote as WebDAV, for files to be streamed by a Spatiand
//	                    application and for a native application to reach a remote as a
//	                    network folder
//
// Nothing is kept on disk but the list of remotes: rclone's file cache is off, so a file is
// read from its remote as it is wanted and written to it as it is written.
//
// Where it is to be found -- its port and token -- is written to
// $XDG_RUNTIME_DIR/spatiand-storage.json (the temporary folder if there is none) for the
// applications to read. A second start finds the first running and goes.
package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"golang.org/x/net/webdav"

	_ "github.com/rclone/rclone/backend/all"   // every backend rclone has
	_ "github.com/rclone/rclone/fs/operations" // operations/* calls
	_ "github.com/rclone/rclone/fs/sync"       // sync/* calls

	"github.com/rclone/rclone/fs"
	"github.com/rclone/rclone/fs/cache"
	"github.com/rclone/rclone/fs/config"
	"github.com/rclone/rclone/librclone/librclone"
	"github.com/rclone/rclone/vfs"
	"github.com/rclone/rclone/vfs/vfscommon"
)

// The port asked for first, so that a network folder added to a native file manager keeps
// working from one session to the next; any free one if it is taken.
const preferredPort = 47731

type place struct {
	Port  int    `json:"port"`
	Token string `json:"token"`
	Pid   int    `json:"pid"`
	// The address a native application is given, with the user name and token in it.
	Dav string `json:"dav"`
}

func runtimeFile() string {
	dir := os.Getenv("XDG_RUNTIME_DIR")
	if dir == "" {
		dir = os.TempDir()
	}
	return filepath.Join(dir, "spatiand-storage.json")
}

// Where Spatiand keeps every application's settings (spatiand_app::config): `apps/` is what
// every paired machine shares, `local/` this machine's own.
func spatiandConfig() string {
	base := os.Getenv("XDG_CONFIG_HOME")
	if base == "" || !filepath.IsAbs(base) {
		home, err := os.UserHomeDir()
		if err != nil || home == "" {
			home = os.TempDir()
		}
		base = filepath.Join(home, ".config")
	}
	if named := os.Getenv("SPATIAND_APPS_CONFIG"); named != "" {
		return named
	}
	return filepath.Join(base, "spatiand")
}

// The remotes: shared with every paired machine. Moved here from where an earlier build kept
// them, the first time.
func configPath() string {
	if named := os.Getenv("SPATIAND_STORAGE_CONFIG"); named != "" {
		return named
	}
	path := filepath.Join(spatiandConfig(), "apps", "storage", "remotes.conf")
	adopt(path, filepath.Join(spatiandConfig(), "rclone.conf"))
	return path
}

// The token native applications are let in with: this machine's own.
func tokenPath() string {
	if named := os.Getenv("SPATIAND_STORAGE_CONFIG"); named != "" {
		return filepath.Join(filepath.Dir(named), "storage.token")
	}
	path := filepath.Join(spatiandConfig(), "local", "storage.token")
	adopt(path, filepath.Join(spatiandConfig(), "storage.token"))
	return path
}

func adopt(path, legacy string) {
	os.MkdirAll(filepath.Dir(path), 0o700)
	if _, err := os.Stat(path); os.IsNotExist(err) {
		if _, err := os.Stat(legacy); err == nil {
			os.Rename(legacy, path)
		}
	}
}

// alreadyRunning says whether the service named in the runtime file answers.
func alreadyRunning() bool {
	raw, err := os.ReadFile(runtimeFile())
	if err != nil {
		return false
	}
	var p place
	if json.Unmarshal(raw, &p) != nil || p.Port == 0 {
		return false
	}
	client := http.Client{Timeout: time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/ping", p.Port), nil)
	req.SetBasicAuth("spatiand", p.Token)
	resp, err := client.Do(req)
	if err != nil {
		return false
	}
	resp.Body.Close()
	return resp.StatusCode == 200
}

// Each remote's files, as rclone's virtual file system over it: made when first asked for,
// forgotten when the remotes change.
type vfses struct {
	sync.Mutex
	byRemote map[string]*vfs.VFS
}

func (v *vfses) get(ctx context.Context, remote string) (*vfs.VFS, error) {
	v.Lock()
	defer v.Unlock()
	if got, ok := v.byRemote[remote]; ok {
		return got, nil
	}
	if _, err := config.FileGetValue(remote, "type"); !err {
		return nil, fs.ErrorDirNotFound
	}
	f, err := cache.Get(ctx, remote+":")
	if err != nil && !errors.Is(err, fs.ErrorIsFile) {
		return nil, err
	}
	opt := vfscommon.Opt
	// No copy of anything on disk: read from the remote as it is read, written to it as it
	// is written.
	opt.CacheMode = vfscommon.CacheModeOff
	opt.DirCacheTime = fs.Duration(30 * time.Second)
	made := vfs.New(f, &opt)
	v.byRemote[remote] = made
	return made, nil
}

func (v *vfses) forget() {
	v.Lock()
	defer v.Unlock()
	for _, each := range v.byRemote {
		each.Shutdown()
	}
	v.byRemote = map[string]*vfs.VFS{}
	cache.Clear()
}

// davFS is one remote as a webdav.FileSystem.
type davFS struct{ vfs *vfs.VFS }

func (d davFS) Mkdir(ctx context.Context, name string, perm os.FileMode) error {
	return d.vfs.Mkdir(name, perm)
}

func (d davFS) OpenFile(ctx context.Context, name string, flag int, perm os.FileMode) (webdav.File, error) {
	h, err := d.vfs.OpenFile(name, flag, perm)
	if err != nil {
		return nil, err
	}
	return h, nil
}

func (d davFS) RemoveAll(ctx context.Context, name string) error {
	node, err := d.vfs.Stat(name)
	if err != nil {
		return err
	}
	return node.RemoveAll()
}

func (d davFS) Rename(ctx context.Context, oldName, newName string) error {
	return d.vfs.Rename(oldName, newName)
}

func (d davFS) Stat(ctx context.Context, name string) (os.FileInfo, error) {
	return d.vfs.Stat(name)
}

type service struct {
	token string
	vfses vfses
	locks webdav.LockSystem
}

func (s *service) authorised(r *http.Request) bool {
	if user, pass, ok := r.BasicAuth(); ok && user == "spatiand" && pass == s.token {
		return true
	}
	return r.URL.Query().Get("token") == s.token
}

// The remotes in the user's own rclone, copied in: a remote set up once with `rclone config`
// is not set up again. Those already here by the same name are left as they are.
func importRemotes(from string) ([]string, error) {
	raw, err := os.ReadFile(from)
	if err != nil {
		return nil, err
	}
	var copied []string
	section := ""
	values := map[string]map[string]string{}
	order := []string{}
	for _, line := range strings.Split(string(raw), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") || strings.HasPrefix(line, ";") {
			continue
		}
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") {
			section = strings.TrimSpace(line[1 : len(line)-1])
			if _, ok := values[section]; !ok {
				values[section] = map[string]string{}
				order = append(order, section)
			}
			continue
		}
		if section == "" {
			continue
		}
		if key, value, ok := strings.Cut(line, "="); ok {
			values[section][strings.TrimSpace(key)] = strings.TrimSpace(value)
		}
	}
	have := map[string]bool{}
	for _, name := range config.FileSections() {
		have[name] = true
	}
	for _, name := range order {
		if have[name] || values[name]["type"] == "" {
			continue
		}
		keys := make([]string, 0, len(values[name]))
		for key := range values[name] {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		for _, key := range keys {
			config.FileSetValue(name, key, values[name][key])
		}
		copied = append(copied, name)
	}
	if len(copied) > 0 {
		config.SaveConfig()
	}
	return copied, nil
}

func (s *service) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !s.authorised(r) {
		w.Header().Set("WWW-Authenticate", `Basic realm="Spatiand storage"`)
		http.Error(w, "who are you?", http.StatusUnauthorized)
		return
	}
	switch {
	case r.URL.Path == "/ping":
		io.WriteString(w, "spatiand-storage\n")
	case r.URL.Path == "/import":
		home, _ := os.UserHomeDir()
		from := r.URL.Query().Get("from")
		if from == "" {
			from = filepath.Join(home, ".config", "rclone", "rclone.conf")
		}
		copied, err := importRemotes(from)
		w.Header().Set("Content-Type", "application/json")
		if err != nil {
			w.WriteHeader(http.StatusNotFound)
			json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
			return
		}
		s.vfses.forget()
		if copied == nil {
			copied = []string{}
		}
		json.NewEncoder(w).Encode(map[string][]string{"copied": copied})
	case strings.HasPrefix(r.URL.Path, "/rc/"):
		method := strings.TrimPrefix(r.URL.Path, "/rc/")
		body, _ := io.ReadAll(io.LimitReader(r.Body, 16<<20))
		input := strings.TrimSpace(string(body))
		if input == "" {
			input = "{}"
		}
		out, status := librclone.RPC(method, input)
		// A remote set up, changed or deleted: what was made of the old list is stale.
		if strings.HasPrefix(method, "config/") && method != "config/listremotes" && method != "config/providers" && method != "config/dump" && method != "config/get" {
			s.vfses.forget()
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		io.WriteString(w, out)
	case strings.HasPrefix(r.URL.Path, "/dav/"):
		rest := strings.TrimPrefix(r.URL.Path, "/dav/")
		remote, _, _ := strings.Cut(rest, "/")
		if remote == "" {
			s.listRemotesAsDav(w, r)
			return
		}
		v, err := s.vfses.get(r.Context(), remote)
		if err != nil {
			http.Error(w, err.Error(), http.StatusNotFound)
			return
		}
		handler := &webdav.Handler{Prefix: "/dav/" + remote, FileSystem: davFS{v}, LockSystem: s.locks, Logger: func(r *http.Request, err error) {
			if err != nil && !errors.Is(err, os.ErrNotExist) {
				log.Printf("%s %s: %v", r.Method, r.URL.Path, err)
			}
		}}
		handler.ServeHTTP(w, r)
	default:
		http.NotFound(w, r)
	}
}

// The top of the network folder: each remote, as a folder.
func (s *service) listRemotesAsDav(w http.ResponseWriter, r *http.Request) {
	names := config.FileSections()
	sort.Strings(names)
	if r.Method == "PROPFIND" {
		w.Header().Set("Content-Type", "application/xml; charset=utf-8")
		w.WriteHeader(http.StatusMultiStatus)
		var b strings.Builder
		b.WriteString(`<?xml version="1.0" encoding="utf-8"?><D:multistatus xmlns:D="DAV:">`)
		entry := func(href, name string) {
			fmt.Fprintf(&b, `<D:response><D:href>%s</D:href><D:propstat><D:prop><D:displayname>%s</D:displayname><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>`, href, name)
		}
		entry("/dav/", "Spatiand")
		if r.Header.Get("Depth") != "0" {
			for _, name := range names {
				entry("/dav/"+name+"/", name)
			}
		}
		b.WriteString(`</D:multistatus>`)
		io.WriteString(w, b.String())
		return
	}
	if r.Method == "OPTIONS" {
		w.Header().Set("DAV", "1, 2")
		w.Header().Set("Allow", "OPTIONS, PROPFIND, GET")
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	io.WriteString(w, "<!doctype html><title>Spatiand storage</title><h1>Remotes</h1><ul>")
	for _, name := range names {
		fmt.Fprintf(w, `<li><a href="/dav/%s/">%s</a></li>`, name, name)
	}
	io.WriteString(w, "</ul>")
}

func main() {
	if alreadyRunning() {
		fmt.Println("spatiand-storage is already running")
		return
	}
	if err := os.MkdirAll(filepath.Dir(configPath()), 0o700); err != nil {
		log.Fatalf("no place for the remotes: %v", err)
	}
	// rclone's config is ours, not the user's own rclone's: their remotes are copied in only
	// when they ask (/import).
	if err := config.SetConfigPath(configPath()); err != nil {
		log.Fatalf("the remotes' file: %v", err)
	}
	librclone.Initialize()

	// The same token from one start to the next, so that a network folder a native file
	// manager has remembered goes on opening.
	tokenFile := tokenPath()
	token := ""
	if kept, err := os.ReadFile(tokenFile); err == nil && len(strings.TrimSpace(string(kept))) >= 16 {
		token = strings.TrimSpace(string(kept))
	} else {
		raw := make([]byte, 16)
		if _, err := rand.Read(raw); err != nil {
			log.Fatalf("no token: %v", err)
		}
		token = hex.EncodeToString(raw)
		os.WriteFile(tokenFile, []byte(token+"\n"), 0o600)
	}
	listener, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", preferredPort))
	if err != nil {
		listener, err = net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			log.Fatalf("nowhere to listen: %v", err)
		}
	}
	port := listener.Addr().(*net.TCPAddr).Port
	where := place{Port: port, Token: token, Pid: os.Getpid(), Dav: fmt.Sprintf("http://spatiand:%s@127.0.0.1:%d/dav/", token, port)}
	encoded, _ := json.Marshal(where)
	file := runtimeFile()
	if err := os.WriteFile(file+".new", encoded, 0o600); err == nil {
		os.Rename(file+".new", file)
	}
	defer os.Remove(file)
	log.Printf("spatiand-storage on 127.0.0.1:%d, remotes in %s", port, configPath())
	s := &service{token: token, vfses: vfses{byRemote: map[string]*vfs.VFS{}}, locks: webdav.NewMemLS()}
	server := &http.Server{Handler: s, ReadHeaderTimeout: 10 * time.Second}
	if err := server.Serve(listener); err != nil {
		log.Fatal(err)
	}
}
