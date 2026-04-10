package inputs_stores

type GetDefaultAccessInput struct{}

func (d GetDefaultAccessInput) GetData() any {
	return "dummy"
}

func (d GetDefaultAccessInput) GetStoreId() string {
	return ""
}

func (d GetDefaultAccessInput) Origin() string {
	return ""
}
