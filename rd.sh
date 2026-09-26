while IFS= read -r line; do
	echo $line
	sleep 0.005 
done < $1

