# Phase4 Lifecycle Diagrams

## Startup Lifecycle

```mermaid
%%{init: {
	"theme": "base",
	"themeVariables": {
		"background": "#ffffff",
		"fontFamily": "monospace",
		"fontSize": "16px",
		"lineColor": "#000",
		"primaryBorderColor": "#000",
		"primaryColor": "#fff",
		"primaryTextColor": "#000",
		"secondaryColor": "#aaa",
		"tertiaryColor": "#ccc"
	},
	"flowchart": {
		"curve": "basis",
		"htmlLabels": true
	}
}}%%
flowchart TD
		A[main] --> B[Parse CLI args]
		B --> C{--list?}
		C -->|yes| D[List input devices and exit]
		C -->|no| E[Build AppConfig from args]
		E --> F{Calibration mode?}
		F -->|yes| G[Use synthetic specs 44.1kHz, 2ch]
		F -->|no| H[Resolve input device and native specs]
		G --> I[Create analyse ring buffer]
		H --> I
		I --> J[Create watch channels: RawPayload and DisplayPayload JSON]
		J --> K{Input source}
		K -->|calibration| M[Spawn generator thread]
		K -->|hardware| L[Start CPAL stream callback]
		L --> N[Spawn analyser thread]
		M --> N
		N --> O[Spawn mapper thread]
		O --> P[Spawn WebSocket server thread]
		P --> Q{--osc flag?}
		Q -->|yes| Q2[Spawn OSC sender thread]
		Q -->|no| R[Run controller loop until shutdown]
		Q2 --> R
		R --> S[Shutdown: drop input, signal keep_running=false, join workers with timeouts]
```

## Runtime Data Flow

```mermaid
%%{init: {
	"theme": "base",
	"themeVariables": {
		"background": "#ffffff",
		"fontFamily": "monospace",
		"fontSize": "16px",
		"lineColor": "#000",
		"primaryBorderColor": "#000",
		"primaryColor": "#fff",
		"primaryTextColor": "#000",
		"secondaryColor": "#aaa",
		"tertiaryColor": "#ccc"
	},
	"flowchart": {
		"curve": "basis",
		"htmlLabels": true
	}
}}%%
flowchart LR
		subgraph Source
			A1[CPAL input callback]:::source
			A2[Generator thread]:::source
		end

		A1 -->|f32 interleaved frames| B2[(Analyse ring buffer)]
		A2 -->|synthetic f32 frames| B2

		B2 --> C2[Analyser thread]
		C2 -->|watch send_replace RawPayload| E[(RawPayload watch)]
		E --> F[Mapper thread]
		F -->|map bins plus JSON serialise| G[(Display JSON watch)]
		F -->|typed DisplayPayload, if --osc| K[(OSC display watch)]
		G --> H[WebSocket server]
		H --> I[Clients: browser or native]
		K --> L[OSC sender thread]
		L --> M[UDP target]

		J[Controller thread] -->|toggle atomics| C2
		J -->|toggle atomics| F
		J -->|toggle atomics| H

```

## Controller State Lifecycle

```mermaid
%%{init: {
	"theme": "base",
	"themeVariables": {
		"background": "#ffffff",
		"fontFamily": "monospace",
		"fontSize": "16px",
		"lineColor": "#000",
		"primaryBorderColor": "#000",
		"primaryColor": "#fff",
		"primaryTextColor": "#000",
		"secondaryColor": "#aaa",
		"tertiaryColor": "#ccc"
	}
}}%%
stateDiagram-v2
		[*] --> Running

		state Running {
			[*] --> Flags
			Flags --> Flags: B key toggles is_broadcasting_websocket
			Flags --> Flags: A key toggles is_analysing
			Flags --> ExitRequested: Ctrl+C sets keep_running=false
		}

		ExitRequested --> Shutdown
		Shutdown --> [*]
```

## Shutdown Ordering and Timeouts

```mermaid
sequenceDiagram
		participant Main as App.shutdown
		participant In as Input stream/device
		participant Gen as Generator
		participant An as Analyser
		participant Map as Mapper
		participant Srv as Server
		participant Osc as OscSender

		Main->>In: Drop input handle
		Main->>Main: keep_running=false

		Main->>Gen: join with 250ms timeout
		Main->>An: join with 1000ms timeout
		Main->>Map: join with 1000ms timeout
		Main->>Srv: join with 1500ms timeout
		Main->>Osc: join with 1500ms timeout

		note over Main,Srv: Server notifies clients, sends close frames, bounded task join
		note over Main,Osc: OSC sender exits when display channel closes after mapper exits
```
