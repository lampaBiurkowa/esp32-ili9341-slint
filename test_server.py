from flask import Flask, request, jsonify

app = Flask(__name__)

@app.route("/api/Tags/tag-crime", methods=["GET", "POST", "PUT", "PATCH", "DELETE"])
def tag_crime():
    return jsonify({
        "method": request.method,
        "path": request.path,
        "headers": dict(request.headers),
        "body": request.get_data(as_text=True),
    })

if __name__ == "__main__":
    app.run(host="0.0.0.0", port=80)
