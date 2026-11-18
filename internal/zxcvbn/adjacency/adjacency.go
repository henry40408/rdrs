package adjacency

type Graph struct {
	Graph map[string][]string
	Name  string
	// AverageDegree is truncated to a whole number, as Rust zxcvbn 3.1.1
	// does; the spatial guess estimate raises it to a power, so the
	// fraction would change scores.
	AverageDegree int
}

// Graphs exports the adjacency graphs by name
var Graphs = make(map[string]*Graph)

func init() {
	initGraph("qwerty", adjacencyGraphQwerty)
	initGraph("dvorak", adjacencyGraphDvorak)
	initGraph("keypad", adjacencyGraphKeypad)
	initGraph("mac_keypad", adjacencyGraphMacKeypad)
}

func initGraph(name string, data map[string][]string) {
	Graphs[name] = &Graph{
		Name:          name,
		Graph:         data,
		AverageDegree: calculateAvgDegree(data),
	}
}

func calculateAvgDegree(g map[string][]string) int {
	neighbours := 0
	for _, value := range g {
		for _, chars := range value {
			if chars != "" {
				neighbours++
			}
		}
	}
	return neighbours / len(g)
}
