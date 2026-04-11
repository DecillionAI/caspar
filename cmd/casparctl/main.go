package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"
)

var dockerNamePattern = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9_.-]*$`)

const (
	nameFileName   = ".casparctl-name"
	trendMaxPoints = 30
)

type dockerStats struct {
	Container string `json:"Container"`
	Name      string `json:"Name"`
	ID        string `json:"ID"`
	CPUPerc   string `json:"CPUPerc"`
	MemUsage  string `json:"MemUsage"`
	MemPerc   string `json:"MemPerc"`
	NetIO     string `json:"NetIO"`
	BlockIO   string `json:"BlockIO"`
	PIDs      string `json:"PIDs"`
}

type inspectInfo struct {
	Name         string
	Image        string
	State        string
	Health       string
	StartedAt    string
	CreatedAt    string
	RestartCount int
	Ports        []string
	Mounts       []string
}

type telemetrySnapshot struct {
	Timestamp       string                 `json:"timestamp"`
	UptimeSec       int64                  `json:"uptime_sec"`
	Node            map[string]interface{} `json:"node"`
	Chain           map[string]interface{} `json:"chain"`
	Federation      map[string]interface{} `json:"federation"`
	Clients         map[string]interface{} `json:"clients"`
	ProtocolTraffic map[string]interface{} `json:"protocol_traffic"`
	VMs             map[string]interface{} `json:"vms"`
	Machines        map[string]interface{} `json:"machines"`
	Costs           map[string]interface{} `json:"costs"`
	Transactions    map[string]interface{} `json:"transactions"`
	Packets         map[string]interface{} `json:"packets"`
	Messages        map[string]interface{} `json:"messages"`
	Creatures       map[string]interface{} `json:"creatures"`
	Validators      map[string]interface{} `json:"validators"`
	Staking         map[string]interface{} `json:"staking"`
	Election        map[string]interface{} `json:"election"`
}

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	switch os.Args[1] {
	case "install":
		must(runInstall(os.Args[2:]))
	case "uninstall":
		must(runUninstall(os.Args[2:]))
	case "purge":
		must(runPurge(os.Args[2:]))
	case "start":
		must(runStart(os.Args[2:]))
	case "pause":
		must(runPause(os.Args[2:]))
	case "resume":
		must(runResume(os.Args[2:]))
	case "stop":
		must(runStop(os.Args[2:]))
	case "stats":
		must(runStats(os.Args[2:]))
	case "help", "-h", "--help":
		printUsage()
	default:
		fmt.Fprintf(os.Stderr, "unknown command %q\n\n", os.Args[1])
		printUsage()
		os.Exit(1)
	}
}

func must(err error) {
	if err == nil {
		return
	}
	fmt.Fprintln(os.Stderr, "error:", err)
	os.Exit(1)
}

func printUsage() {
	fmt.Println(`casparctl - manage Caspar as a Dockerized node

Usage:
  casparctl <command> [flags]

Commands:
  install    Build image, create volumes, and run Caspar with one command
  uninstall  Stop and remove the Caspar container
  purge      Uninstall + remove image and volumes
  start      Start the Caspar container
  pause      Pause the Caspar container
  resume     Resume (unpause) the Caspar container
  stop       Stop the Caspar container
  stats      Realtime multi-section dashboard for Caspar container stats

Run "casparctl <command> --help" for command-specific flags.`)
}

func runInstall(args []string) error {
	fs := flag.NewFlagSet("install", flag.ContinueOnError)
	projectDir := fs.String("project-dir", "", "path to Caspar node directory (auto-detected when omitted)")
	envFile := fs.String("env-file", ".env", "environment file relative to project-dir")
	name := fs.String("name", "", "name to use for both docker image and container")
	if err := fs.Parse(args); err != nil {
		return err
	}

	if err := requireDocker(); err != nil {
		return err
	}

	absProject, err := resolveProjectDir(*projectDir)
	if err != nil {
		return err
	}

	if strings.TrimSpace(*name) == "" {
		return errors.New("--name is required for install (this name is used for image + container)")
	}
	if err := validateDockerName(*name); err != nil {
		return fmt.Errorf("invalid --name value: %w", err)
	}

	dockerfile := filepath.Join(absProject, "Dockerfile")
	if _, err := os.Stat(dockerfile); err != nil {
		return fmt.Errorf("Dockerfile not found at %s", dockerfile)
	}

	absEnv := filepath.Join(absProject, *envFile)
	if _, err := os.Stat(absEnv); err != nil {
		return fmt.Errorf("env file not found at %s (copy sample.env to .env first)", absEnv)
	}

	fmt.Println("→ Building Caspar Docker image (this may take several minutes)...")
	if err := runCommand("", "docker", "build", "-t", *name, "-f", dockerfile, absProject); err != nil {
		return err
	}

	volumeName := *name + "-storage"
	_ = runCommandQuiet("", "docker", "volume", "create", volumeName)
	_ = runCommandQuiet("", "docker", "rm", "-f", *name)

	fmt.Println("→ Starting Caspar container...")
	if err := runCommand("", "docker", "run", "-d",
		"--name", *name,
		"--restart", "unless-stopped",
		"--env-file", absEnv,
		"-v", volumeName+":/app/storage",
		"-p", "9999:9999",
		"-p", "9099:9099",
		*name,
	); err != nil {
		return err
	}

	if err := writeSavedName(absProject, *name); err != nil {
		return err
	}

	fmt.Printf("✓ Caspar installed and running in container %q\n", *name)
	fmt.Printf("  View live dashboard: casparctl stats\n")
	return nil
}

func runUninstall(args []string) error {
	fs := flag.NewFlagSet("uninstall", flag.ContinueOnError)
	projectDir := fs.String("project-dir", "", "path to Caspar node directory (auto-detected when omitted)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if err := requireDocker(); err != nil {
		return err
	}
	absProject, err := resolveProjectDir(*projectDir)
	if err != nil {
		return err
	}
	name, err := loadSavedName(absProject)
	if err != nil {
		return err
	}
	if !containerExists(name) {
		fmt.Printf("container %q does not exist; nothing to uninstall\n", name)
		return nil
	}
	if err := runCommand("", "docker", "rm", "-f", name); err != nil {
		return err
	}
	fmt.Printf("✓ Uninstalled container %q\n", name)
	return nil
}

func runPurge(args []string) error {
	fs := flag.NewFlagSet("purge", flag.ContinueOnError)
	projectDir := fs.String("project-dir", "", "path to Caspar node directory (auto-detected when omitted)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if err := requireDocker(); err != nil {
		return err
	}
	absProject, err := resolveProjectDir(*projectDir)
	if err != nil {
		return err
	}
	name, err := loadSavedName(absProject)
	if err != nil {
		return err
	}

	_ = runCommandQuiet("", "docker", "rm", "-f", name)
	_ = runCommandQuiet("", "docker", "rmi", name)
	_ = runCommandQuiet("", "docker", "volume", "rm", name+"-storage")

	fmt.Printf("✓ Purged Caspar container %q, image %q, and storage volume\n", name, name)
	return nil
}

func runStart(args []string) error {
	return lifecycleCommand("start", args)
}

func runPause(args []string) error {
	return lifecycleCommand("pause", args)
}

func runResume(args []string) error {
	return lifecycleCommand("unpause", args)
}

func runStop(args []string) error {
	fs := flag.NewFlagSet("stop", flag.ContinueOnError)
	projectDir := fs.String("project-dir", "", "path to Caspar node directory (auto-detected when omitted)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if err := requireDocker(); err != nil {
		return err
	}
	absProject, err := resolveProjectDir(*projectDir)
	if err != nil {
		return err
	}
	name, err := loadSavedName(absProject)
	if err != nil {
		return err
	}
	if err := runCommand("", "docker", "stop", name); err != nil {
		return err
	}
	fmt.Printf("✓ Stopped container %q\n", name)
	return nil
}

func lifecycleCommand(action string, args []string) error {
	fs := flag.NewFlagSet(action, flag.ContinueOnError)
	projectDir := fs.String("project-dir", "", "path to Caspar node directory (auto-detected when omitted)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if err := requireDocker(); err != nil {
		return err
	}
	absProject, err := resolveProjectDir(*projectDir)
	if err != nil {
		return err
	}
	name, err := loadSavedName(absProject)
	if err != nil {
		return err
	}
	if err := runCommand("", "docker", action, name); err != nil {
		return err
	}
	fmt.Printf("✓ %s completed for container %q\n", actionLabel(action), name)
	return nil
}

func actionLabel(action string) string {
	if action == "unpause" {
		return "resume"
	}
	return action
}

func runStats(args []string) error {
	fs := flag.NewFlagSet("stats", flag.ContinueOnError)
	projectDir := fs.String("project-dir", "", "path to Caspar node directory (auto-detected when omitted)")
	interval := fs.Duration("interval", 2*time.Second, "refresh interval")
	logLines := fs.Int("log-lines", 6, "number of recent container logs to show")
	if err := fs.Parse(args); err != nil {
		return err
	}
	if err := requireDocker(); err != nil {
		return err
	}
	absProject, err := resolveProjectDir(*projectDir)
	if err != nil {
		return err
	}
	name, err := loadSavedName(absProject)
	if err != nil {
		return err
	}

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(sigCh)

	ticker := time.NewTicker(*interval)
	defer ticker.Stop()

	cpuHistory := make([]float64, 0, trendMaxPoints)

	render := func() error {
		inspect, err := getInspectInfo(name)
		if err != nil {
			return err
		}

		stats, statsErr := getContainerStats(name)
		if statsErr == nil {
			if cpu, ok := parsePercent(stats.CPUPerc); ok {
				cpuHistory = append(cpuHistory, cpu)
				if len(cpuHistory) > trendMaxPoints {
					cpuHistory = cpuHistory[len(cpuHistory)-trendMaxPoints:]
				}
			}
		}

		logs, _ := getContainerLogs(name, *logLines)
		telemetry, telemetryErr := getTelemetrySnapshot()
		renderDashboard(inspect, stats, statsErr, logs, telemetry, telemetryErr, *interval, cpuHistory)
		return nil
	}

	if err := render(); err != nil {
		return err
	}

	for {
		select {
		case <-sigCh:
			fmt.Println("\nExiting Caspar dashboard.")
			return nil
		case <-ticker.C:
			if err := render(); err != nil {
				return err
			}
		}
	}
}

func getInspectInfo(container string) (*inspectInfo, error) {
	out, err := exec.Command("docker", "inspect", container).CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg == "" {
			msg = err.Error()
		}
		return nil, fmt.Errorf("docker inspect failed: %s", msg)
	}

	var raw []struct {
		Name   string `json:"Name"`
		Config struct {
			Image string `json:"Image"`
		} `json:"Config"`
		Created      string `json:"Created"`
		RestartCount int    `json:"RestartCount"`
		State        struct {
			Status    string `json:"Status"`
			StartedAt string `json:"StartedAt"`
			Health    *struct {
				Status string `json:"Status"`
			} `json:"Health"`
		} `json:"State"`
		HostConfig struct {
			PortBindings map[string][]struct {
				HostIP   string `json:"HostIp"`
				HostPort string `json:"HostPort"`
			} `json:"PortBindings"`
		} `json:"HostConfig"`
		Mounts []struct {
			Source      string `json:"Source"`
			Destination string `json:"Destination"`
		} `json:"Mounts"`
	}

	if err := json.Unmarshal(out, &raw); err != nil {
		return nil, fmt.Errorf("cannot parse docker inspect output: %w", err)
	}
	if len(raw) == 0 {
		return nil, errors.New("docker inspect returned no container data")
	}

	item := raw[0]
	info := &inspectInfo{
		Name:         strings.TrimPrefix(item.Name, "/"),
		Image:        item.Config.Image,
		State:        item.State.Status,
		CreatedAt:    shortTime(item.Created),
		StartedAt:    shortTime(item.State.StartedAt),
		RestartCount: item.RestartCount,
		Health:       "n/a",
	}
	if item.State.Health != nil && item.State.Health.Status != "" {
		info.Health = item.State.Health.Status
	}
	for p, binds := range item.HostConfig.PortBindings {
		if len(binds) == 0 {
			info.Ports = append(info.Ports, p+" (internal)")
			continue
		}
		for _, b := range binds {
			host := b.HostIP
			if host == "" {
				host = "0.0.0.0"
			}
			info.Ports = append(info.Ports, fmt.Sprintf("%s:%s -> %s", host, b.HostPort, p))
		}
	}
	if len(info.Ports) == 0 {
		info.Ports = []string{"none"}
	}
	for _, m := range item.Mounts {
		info.Mounts = append(info.Mounts, fmt.Sprintf("%s -> %s", m.Source, m.Destination))
	}
	if len(info.Mounts) == 0 {
		info.Mounts = []string{"none"}
	}
	return info, nil
}

func getContainerStats(container string) (*dockerStats, error) {
	out, err := exec.Command("docker", "stats", "--no-stream", "--format", "{{json .}}", container).CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg == "" {
			msg = err.Error()
		}
		return nil, fmt.Errorf("docker stats failed: %s", msg)
	}

	stats := &dockerStats{}
	scanner := bufio.NewScanner(bytes.NewReader(out))
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		if err := json.Unmarshal([]byte(line), stats); err != nil {
			return nil, fmt.Errorf("cannot parse docker stats output: %w", err)
		}
		return stats, nil
	}
	if err := scanner.Err(); err != nil {
		return nil, err
	}
	return nil, errors.New("no stats returned (container may be stopped/paused)")
}

func getContainerLogs(container string, lines int) ([]string, error) {
	out, err := exec.Command("docker", "logs", "--tail", strconv.Itoa(lines), container).CombinedOutput()
	if err != nil && len(out) == 0 {
		return nil, err
	}
	res := make([]string, 0)
	scanner := bufio.NewScanner(bytes.NewReader(out))
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		res = append(res, line)
	}
	if len(res) == 0 {
		res = []string{"(no recent logs)"}
	}
	return res, nil
}

func renderDashboard(info *inspectInfo, stats *dockerStats, statsErr error, logs []string, telemetry *telemetrySnapshot, telemetryErr error, interval time.Duration, cpuHistory []float64) {
	clearScreen()
	now := time.Now().Format(time.RFC1123)
	printSection("CASPAR NODE DASHBOARD", []string{
		"Updated: " + now,
		"Refresh: " + interval.String(),
		"Container: " + info.Name,
		"State: " + strings.ToUpper(info.State) + "   Health: " + strings.ToUpper(info.Health),
	})

	printSection("RUNTIME OVERVIEW", []string{
		"Image: " + info.Image,
		"Created: " + info.CreatedAt,
		"Started: " + info.StartedAt,
		fmt.Sprintf("Restart count: %d", info.RestartCount),
	})

	if statsErr != nil {
		printSection("RESOURCE STATS", []string{"Stats unavailable: " + statsErr.Error()})
	} else {
		trend := renderSparkline(cpuHistory)
		printSection("RESOURCE STATS", []string{
			"CPU: " + stats.CPUPerc + "   MEM: " + stats.MemUsage + " (" + stats.MemPerc + ")",
			"NET I/O: " + stats.NetIO,
			"BLOCK I/O: " + stats.BlockIO,
			"PIDs: " + stats.PIDs,
			"CPU trend: " + trend,
		})
	}

	printSection("PORT MAPPINGS", info.Ports)
	if telemetryErr != nil {
		printSection("CASPAR TELEMETRY", []string{"Telemetry unavailable: " + telemetryErr.Error()})
	} else {
		printSection("CASPAR TELEMETRY", telemetryLines(telemetry))
	}
	printSection("MOUNTS", info.Mounts)
	printSection("RECENT LOGS", padLogs(logs, 8))
	printSection("ACTIONS", []string{
		"Start:     casparctl start",
		"Pause:     casparctl pause",
		"Resume:    casparctl resume",
		"Stop:      casparctl stop",
		"Cleanup:   casparctl purge",
	})
	fmt.Println("Press Ctrl+C to exit dashboard")
}

func getTelemetrySnapshot() (*telemetrySnapshot, error) {
	client := &http.Client{Timeout: 1200 * time.Millisecond}
	resp, err := client.Get("http://127.0.0.1:9099/telemetry/snapshot")
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return nil, fmt.Errorf("status %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var snap telemetrySnapshot
	if err := json.NewDecoder(resp.Body).Decode(&snap); err != nil {
		return nil, err
	}
	return &snap, nil
}

func telemetryLines(t *telemetrySnapshot) []string {
	if t == nil {
		return []string{"(no telemetry)"}
	}
	return []string{
		fmt.Sprintf("Uptime: %ds   Snapshot: %s", t.UptimeSec, t.Timestamp),
		"Node: " + compactMap(t.Node),
		"Chain: " + compactMap(t.Chain),
		"Federation: " + compactMap(t.Federation),
		"Clients: " + compactMap(t.Clients),
		"Protocol traffic: " + compactMap(t.ProtocolTraffic),
		"VMs: " + compactMap(t.VMs),
		"Machines: " + compactMap(t.Machines),
		"Costs: " + compactMap(t.Costs),
		"Transactions: " + compactMap(t.Transactions),
		"Packets: " + compactMap(t.Packets),
		"Messages: " + compactMap(t.Messages),
		"Creatures: " + compactMap(t.Creatures),
		"Validators: " + compactMap(t.Validators),
		"Staking: " + compactMap(t.Staking),
		"Election: " + compactMap(t.Election),
	}
}

func compactMap(m map[string]interface{}) string {
	if len(m) == 0 {
		return "{}"
	}
	b, err := json.Marshal(m)
	if err != nil {
		return "{...}"
	}
	return string(b)
}

func printSection(title string, lines []string) {
	width := 104
	fmt.Println("┌" + strings.Repeat("─", width-2) + "┐")
	fmt.Println(boxLine(width, "◆ "+title))
	fmt.Println("├" + strings.Repeat("─", width-2) + "┤")
	if len(lines) == 0 {
		lines = []string{"(empty)"}
	}
	for _, line := range lines {
		fmt.Println(boxLine(width, line))
	}
	fmt.Println("└" + strings.Repeat("─", width-2) + "┘")
}

func padLogs(logs []string, n int) []string {
	if len(logs) >= n {
		return logs[len(logs)-n:]
	}
	out := make([]string, 0, n)
	for i := 0; i < n-len(logs); i++ {
		out = append(out, "")
	}
	return append(out, logs...)
}

func boxLine(width int, content string) string {
	if width < 2 {
		return content
	}
	max := width - 4
	if runeCount(content) > max {
		content = trimRunes(content, max)
	}
	pad := max - runeCount(content)
	return fmt.Sprintf("│ %s%s │", content, strings.Repeat(" ", pad))
}

func runeCount(s string) int {
	return len([]rune(s))
}

func trimRunes(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	if n <= 1 {
		return string(r[:n])
	}
	return string(r[:n-1]) + "…"
}

func renderSparkline(values []float64) string {
	if len(values) == 0 {
		return "(warming up)"
	}
	bars := []rune("▁▂▃▄▅▆▇█")
	max := 0.0
	for _, v := range values {
		if v > max {
			max = v
		}
	}
	if max <= 0 {
		return strings.Repeat("▁", len(values))
	}
	var b strings.Builder
	for _, v := range values {
		idx := int((v / max) * float64(len(bars)-1))
		if idx < 0 {
			idx = 0
		}
		if idx >= len(bars) {
			idx = len(bars) - 1
		}
		b.WriteRune(bars[idx])
	}
	return b.String()
}

func parsePercent(p string) (float64, bool) {
	v := strings.TrimSuffix(strings.TrimSpace(p), "%")
	if v == "" {
		return 0, false
	}
	f, err := strconv.ParseFloat(v, 64)
	if err != nil {
		return 0, false
	}
	return f, true
}

func shortTime(raw string) string {
	if raw == "" || raw == "0001-01-01T00:00:00Z" {
		return "n/a"
	}
	t, err := time.Parse(time.RFC3339Nano, raw)
	if err != nil {
		return raw
	}
	return t.Local().Format("2006-01-02 15:04:05 MST")
}

func clearScreen() {
	if runtime.GOOS == "windows" {
		_ = runCommandQuiet("", "cmd", "/c", "cls")
		return
	}
	fmt.Print("\033[H\033[2J")
}

func requireDocker() error {
	if _, err := exec.LookPath("docker"); err != nil {
		return errors.New("docker is required but was not found in PATH")
	}
	if err := exec.Command("docker", "info").Run(); err != nil {
		return errors.New("docker daemon is not reachable; start Docker first")
	}
	return nil
}

func validateDockerName(name string) error {
	trimmed := strings.TrimSpace(name)
	if trimmed == "" {
		return errors.New("name must not be empty")
	}
	if !dockerNamePattern.MatchString(trimmed) {
		return errors.New("allowed characters are letters, numbers, dot, underscore, and hyphen")
	}
	return nil
}

func nameFilePath(projectDir string) (string, error) {
	absProject, err := filepath.Abs(projectDir)
	if err != nil {
		return "", err
	}
	return filepath.Join(absProject, nameFileName), nil
}

func resolveProjectDir(projectDir string) (string, error) {
	if strings.TrimSpace(projectDir) != "" {
		abs, err := filepath.Abs(projectDir)
		if err != nil {
			return "", err
		}
		if err := validateNodeProjectDir(abs); err != nil {
			return "", err
		}
		return abs, nil
	}

	cwd, _ := os.Getwd()
	exe, _ := os.Executable()
	exeDir := filepath.Dir(exe)

	candidates := []string{
		filepath.Join(cwd, "node"),
		filepath.Join(cwd, "..", "node"),
		filepath.Join(cwd, "..", "..", "node"),
		filepath.Join(exeDir, "node"),
		filepath.Join(exeDir, "..", "node"),
		filepath.Join(exeDir, "..", "..", "node"),
	}

	for _, c := range candidates {
		abs, err := filepath.Abs(c)
		if err != nil {
			continue
		}
		if validateNodeProjectDir(abs) == nil {
			return abs, nil
		}
	}

	return "", errors.New("could not auto-detect node project directory; pass --project-dir explicitly")
}

func validateNodeProjectDir(dir string) error {
	dockerfile := filepath.Join(dir, "Dockerfile")
	if _, err := os.Stat(dockerfile); err != nil {
		return fmt.Errorf("node Dockerfile not found in %s", dir)
	}
	return nil
}

func writeSavedName(projectDir, name string) error {
	if err := validateDockerName(name); err != nil {
		return err
	}
	path, err := nameFilePath(projectDir)
	if err != nil {
		return err
	}
	return os.WriteFile(path, []byte(strings.TrimSpace(name)+"\n"), 0o644)
}

func loadSavedName(projectDir string) (string, error) {
	path, err := nameFilePath(projectDir)
	if err != nil {
		return "", err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("could not read %s; run install first to generate it", path)
	}
	name := strings.TrimSpace(string(data))
	if err := validateDockerName(name); err != nil {
		return "", fmt.Errorf("invalid name stored in %s: %w", path, err)
	}
	return name, nil
}

func containerExists(container string) bool {
	return exec.Command("docker", "container", "inspect", container).Run() == nil
}

func runCommand(workdir, name string, args ...string) error {
	cmd := exec.Command(name, args...)
	if workdir != "" {
		cmd.Dir = workdir
	}
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

func runCommandQuiet(workdir, name string, args ...string) error {
	cmd := exec.Command(name, args...)
	if workdir != "" {
		cmd.Dir = workdir
	}
	return cmd.Run()
}
