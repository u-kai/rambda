package main

import "github.com/aws/aws-lambda-go/lambda"

type Response struct {
	Message string `json:"message"`
	Input   any    `json:"input"`
}

func main() {
	lambda.Start(func(event any) (Response, error) {
		return Response{
			Message: "Hello, World!",
			Input:   event,
		},nil
	})
}
